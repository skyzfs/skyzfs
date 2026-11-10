//! CSI v1 service implementations (auth design §9.3). Controller talks
//! to the SkyNBD control plane (its identity needs the `volume` op
//! grant); Node drives nbd-client through [`crate::node::NodeMechanics`]
//! (its identity needs `enroll`; snapshot ops additionally need
//! `snapshot`). Snapshot restores follow CSI design §4: read-only
//! requests become reference volumes, writable ones become server fast
//! clones (the controller identity then also needs the `clone` op on
//! the source volume). Anything SkyNBD doesn't support yet (expansion,
//! per-node publish bookkeeping beyond stage/unstage) answers
//! `UNIMPLEMENTED`.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use crate::csi::v1 as csi;
use crate::node::{NodeError, NodeMechanics};
use crate::{cp, DRIVER_NAME};

/// Block sizes are 4 KiB-aligned end to end; round requested capacity up.
fn capacity_from_range(range: Option<&csi::CapacityRange>) -> i64 {
    const DEFAULT: i64 = 1 << 30;
    const ALIGN: i64 = 4096;
    let want = match range {
        Some(r) if r.required_bytes > 0 => r.required_bytes,
        Some(r) if r.limit_bytes > 0 => r.limit_bytes,
        _ => DEFAULT,
    };
    (want + ALIGN - 1) / ALIGN * ALIGN
}

/// A thin volume's lineage sibling (CSI 6h-2, V3.2 §6.5): the server
/// volume that receives its fs-consistent snapshots and serves its RO
/// exports and clones — the head volume's bytes are pool-physical and
/// never snapshotted server-natively. CSI volume ids are `pvc-<uuid>`,
/// so the suffix cannot collide with another CSI volume, and the
/// sibling's existence doubles as the "is thin" marker for
/// CreateSnapshot (which only receives a volume id).
pub fn lineage_name(volume: &str) -> String {
    format!("{volume}-lin")
}

fn cp_status(e: cp::CpError) -> Status {
    match e {
        cp::CpError::BadRequest(m) => Status::invalid_argument(m),
        cp::CpError::NotFound(m) => Status::not_found(m),
        cp::CpError::Conflict(m) => Status::already_exists(m),
        cp::CpError::Denied(m) => Status::permission_denied(m),
        cp::CpError::Other(m) => Status::internal(m),
    }
}

fn timestamp_from_millis(time: u64) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: (time / 1_000) as i64,
        nanos: ((time % 1_000) * 1_000_000) as i32,
    }
}

fn node_status(e: NodeError) -> Status {
    match e {
        NodeError::NoFreeDevice => Status::resource_exhausted(e.to_string()),
        NodeError::NotStaged(_) | NodeError::ThinUnavailable => {
            Status::failed_precondition(e.to_string())
        }
        // D-M13: the final publish cycle hasn't landed yet — kubelet
        // should simply retry the unstage (the resumable cycle keeps
        // uploading in the meantime), so signal "not done", not broken.
        NodeError::FinalCycle(_, _) => Status::unavailable(e.to_string()),
        _ => Status::internal(e.to_string()),
    }
}

/// (block?, fstype) from a volume capability; `None` = no access_type set.
fn capability_shape(cap: Option<&csi::VolumeCapability>) -> Option<(bool, Option<String>)> {
    use csi::volume_capability::AccessType;
    match cap?.access_type.as_ref()? {
        AccessType::Block(_) => Some((true, None)),
        AccessType::Mount(m) => Some((false, (!m.fs_type.is_empty()).then(|| m.fs_type.clone()))),
    }
}

/// A StorageClass `mountOptions` entry asking the guest filesystem to
/// issue discards, in the spellings mount accepts. `discard` (and ext4's
/// `discard=async`) turn on inline trim; `nodiscard` is the default and
/// is fine.
///
/// An ENTRY is not an option: `mountOptions: ["noatime,discard"]` is
/// legal YAML and the form a hand-written StorageClass most often takes,
/// because that is what one would pass to `mount -o`. Splitting only on
/// `=` let exactly that spelling through the refusal, which is the one
/// most likely to be typed rather than generated.
fn is_discard_mount_option(opt: &str) -> bool {
    opt.split(',')
        .any(|o| o.split('=').next().unwrap_or(o).trim() == "discard")
}

/// Invariant B: refuse `mountOptions` that would arm guest discard.
///
/// At NodeStageVolume this makes an EXISTING PV carrying the option
/// unstageable after an upgrade to 15j, and that is intended: the
/// option was inert before the check and is inert after it, so nothing
/// that used to work stops working — what changes is that the volume
/// now says so instead of mounting as if trim were on. The remedy is to
/// drop `discard` from the PV's `spec.mountOptions` (and from the
/// StorageClass, or the next PVC provisions the same PV again), after
/// which the stage proceeds unchanged.
///
/// The node never passes `mount_flags` to `mount(8)` at all, so such an
/// option is *already* inert — and that is precisely the problem worth
/// an error rather than silence. An operator who sets it believes trim
/// is on; the truthful answer is that this driver cannot honor it,
/// because a guest discard on the shim unmaps blocks the external origin
/// still serves and diverges the served image from the published
/// generation in both directions (which is why the shim's queue is
/// masked at every stage, retarget and cycle). Failing CreateVolume says
/// so at the one moment an operator is watching.
///
/// Returns the refusal text, so both call sites word it identically.
fn guest_discard_refusal(caps: &[csi::VolumeCapability]) -> Option<String> {
    use csi::volume_capability::AccessType;
    for c in caps {
        let Some(AccessType::Mount(m)) = c.access_type.as_ref() else {
            continue;
        };
        if let Some(opt) = m.mount_flags.iter().find(|o| is_discard_mount_option(o)) {
            return Some(format!(
                "mountOptions {opt:?} is not supported: guest-initiated discard on an \
                 origin-backed volume unmaps blocks the published generation still \
                 serves, so the shim's discard support is masked by construction \
                 (Invariant B). Remove it — the agent reclaims pool space itself on \
                 every advance."
            ));
        }
    }
    None
}

/// True when every requested capability is a reader-only access mode —
/// the shape that selects a reference volume for snapshot sources.
fn all_read_only(caps: &[csi::VolumeCapability]) -> bool {
    use csi::volume_capability::access_mode::Mode;
    !caps.is_empty()
        && caps.iter().all(|c| {
            c.access_mode.as_ref().is_some_and(|m| {
                m.mode == Mode::SingleNodeReaderOnly as i32
                    || m.mode == Mode::MultiNodeReaderOnly as i32
            })
        })
}

// ---------------------------------------------------------------- identity

pub struct IdentitySvc {
    pub with_controller: bool,
}

#[tonic::async_trait]
impl csi::identity_server::Identity for IdentitySvc {
    async fn get_plugin_info(
        &self,
        _: Request<csi::GetPluginInfoRequest>,
    ) -> Result<Response<csi::GetPluginInfoResponse>, Status> {
        Ok(Response::new(csi::GetPluginInfoResponse {
            name: DRIVER_NAME.into(),
            vendor_version: env!("CARGO_PKG_VERSION").into(),
            manifest: Default::default(),
        }))
    }

    async fn get_plugin_capabilities(
        &self,
        _: Request<csi::GetPluginCapabilitiesRequest>,
    ) -> Result<Response<csi::GetPluginCapabilitiesResponse>, Status> {
        use csi::plugin_capability::{service, Service, Type};
        let mut capabilities = vec![
            // Without this the external-provisioner never builds
            // accessibility_requirements — thin volumes (CSI design 6h)
            // depend on them to pin to their cache-file node. Raw
            // volumes return no accessible_topology and stay free.
            csi::PluginCapability {
                r#type: Some(Type::Service(Service {
                    r#type: service::Type::VolumeAccessibilityConstraints as i32,
                })),
            },
        ];
        if self.with_controller {
            capabilities.push(csi::PluginCapability {
                r#type: Some(Type::Service(Service {
                    r#type: service::Type::ControllerService as i32,
                })),
            });
        }
        Ok(Response::new(csi::GetPluginCapabilitiesResponse {
            capabilities,
        }))
    }

    async fn probe(
        &self,
        _: Request<csi::ProbeRequest>,
    ) -> Result<Response<csi::ProbeResponse>, Status> {
        Ok(Response::new(csi::ProbeResponse { ready: Some(true) }))
    }
}

// -------------------------------------------------------------- controller

pub struct ControllerSvc {
    pub cp: cp::Client,
}

#[tonic::async_trait]
impl csi::controller_server::Controller for ControllerSvc {
    async fn create_volume(
        &self,
        req: Request<csi::CreateVolumeRequest>,
    ) -> Result<Response<csi::CreateVolumeResponse>, Status> {
        let req = req.into_inner();
        if req.name.is_empty() {
            return Err(Status::invalid_argument("name is required"));
        }
        // StorageClass `mode` parameter (CSI design §2): raw (default)
        // attaches the head directly; thin (origin-advance §5) stages a
        // publish-only local thin pool on the node.
        let thin = match req.parameters.get("mode").map(String::as_str) {
            None | Some("raw") => false,
            Some("thin") => true,
            Some(other) => {
                return Err(Status::invalid_argument(format!(
                    "unknown mode {other:?} (raw|thin)"
                )))
            }
        };
        // Invariant B: `mountOptions: [discard]` is dropped on the floor
        // by the node, and an operator who set it must hear that from
        // the PVC rather than from a trim that quietly never happened.
        if let Some(why) = guest_discard_refusal(&req.volume_capabilities) {
            return Err(Status::invalid_argument(why));
        }
        // §7 compaction-policy dials (task 15i), validated strictly at
        // provisioning time so a bad StorageClass fails the PVC, not a
        // stage retry loop.
        let reclaim = crate::agentcycle::ReclaimParams::from_map(&req.parameters)
            .map_err(Status::invalid_argument)?;
        if !thin && !reclaim.is_empty() {
            // The dials configure the embedded cycle's reclaim policy;
            // a raw volume has no cycle — reject rather than silently
            // ignore.
            return Err(Status::invalid_argument(
                "reclaim parameters (reclaimStrategy, reclaimTiming, \
                 reclaimRangeThreshold, swingEveryNAdvances) require mode: thin",
            ));
        }
        // Same reasoning one level down: a thin BLOCK volume has no
        // filesystem to freeze, so the stage starts no cycle at all
        // (`start_cycle`'s block guard) and every dial is inert. The
        // access type is in the request, so this is a PVC-time failure
        // rather than a policy nobody ever runs.
        if thin
            && !reclaim.is_empty()
            && req
                .volume_capabilities
                .iter()
                .any(|c| capability_shape(Some(c)).is_some_and(|(block, _)| block))
        {
            return Err(Status::invalid_argument(
                "reclaim parameters (reclaimStrategy, reclaimTiming, \
                 reclaimRangeThreshold, swingEveryNAdvances) require a \
                 filesystem volume — block volumes run no snapshot cycle",
            ));
        }
        if thin && req.volume_content_source.is_some() {
            // A thin head's server-side bytes are pool-physical (V3.2
            // §15.7): its content cannot come from (or be) a server
            // snapshot until logical snapshots flow through the lineage
            // volume (design §3, item 6h-2). Restore into a raw-mode
            // StorageClass instead.
            return Err(Status::invalid_argument(
                "snapshot-sourced volumes are not supported in thin mode yet; \
                 restore via a raw-mode StorageClass",
            ));
        }
        // Snapshot-sourced volumes (CSI design §4): a read-only request
        // becomes a *reference volume* onto the snapshot's existing RO
        // export — no server volume exists or is created, so this is O(1)
        // and DeleteVolume has nothing to undo. A writable request becomes
        // a server fast clone (design §4.1): a real volume starting at the
        // snapshot's content, diverging independently.
        if let Some(src) = req.volume_content_source.as_ref() {
            use csi::volume_content_source::Type;
            let Some(Type::Snapshot(s)) = src.r#type.as_ref() else {
                return Err(Status::invalid_argument(
                    "only snapshot content sources are supported",
                ));
            };
            let Some((vol, snap)) = s.snapshot_id.split_once('/') else {
                return Err(Status::not_found(format!(
                    "unknown snapshot {:?}",
                    s.snapshot_id
                )));
            };
            if all_read_only(&req.volume_capabilities) {
                return Ok(Response::new(csi::CreateVolumeResponse {
                    volume: Some(csi::Volume {
                        volume_id: format!("ro:{vol}/snap/{snap}"),
                        // 0 = capacity unspecified; the snapshot dictates it.
                        capacity_bytes: 0,
                        content_source: req.volume_content_source.clone(),
                        ..Default::default()
                    }),
                }));
            }
            // Clone size: 0 lets the server inherit the source volume's
            // size (the usual restore, where the PVC just echoes it); an
            // explicit request may only grow — the server 400s below the
            // source, which is CSI's OUT_OF_RANGE.
            let size = match req.capacity_range.as_ref() {
                Some(r) if r.required_bytes > 0 || r.limit_bytes > 0 => {
                    capacity_from_range(req.capacity_range.as_ref()) as u64
                }
                _ => 0,
            };
            let from = s.snapshot_id.clone();
            let vol = match self.cp.create_volume(&req.name, size, Some(&from)).await {
                Ok(v) => v,
                Err(cp::CpError::BadRequest(m)) => return Err(Status::out_of_range(m)),
                Err(e) => return Err(cp_status(e)),
            };
            return Ok(Response::new(csi::CreateVolumeResponse {
                volume: Some(csi::Volume {
                    volume_id: vol.name,
                    capacity_bytes: vol.size as i64,
                    content_source: req.volume_content_source.clone(),
                    ..Default::default()
                }),
            }));
        }
        let mut size = capacity_from_range(req.capacity_range.as_ref()) as u64;
        let mut accessible_topology = Vec::new();
        if thin {
            // The origin must be whole thin chunks. 1 MiB is a multiple
            // of every supported chunk size (node default 64 KiB), so
            // rounding here keeps the controller node-config-agnostic.
            size = size.next_multiple_of(1 << 20);
            // Thin volumes are node-sticky: the volume's logical
            // structure (thin metadata) lives in that node's cache file
            // (V3.2 §6.5), and rehydrating elsewhere needs the lineage
            // flow (6h-2). Pin the PV to the scheduler-selected node —
            // which requires late binding.
            let node = req
                .accessibility_requirements
                .as_ref()
                .and_then(|t| t.preferred.first().or_else(|| t.requisite.first()))
                .cloned()
                .ok_or_else(|| {
                    Status::invalid_argument(
                        "thin mode requires topology (set the StorageClass to \
                         volumeBindingMode: WaitForFirstConsumer)",
                    )
                })?;
            accessible_topology.push(node);
        }
        // The server's create is idempotent: identical replay returns the
        // existing volume (200), a different size conflicts (409) — which
        // is exactly CSI's ALREADY_EXISTS contract.
        let vol = self
            .cp
            .create_volume(&req.name, size, None)
            .await
            .map_err(cp_status)?;
        // The node learns the data-path mode (and, for thin, the origin
        // size the stack must present) from the volume context — it
        // persists on the PV and arrives in every NodeStageVolume.
        let volume_context = if thin {
            // The lineage sibling (V3.2 §6.5) receives the embedded
            // agent's fs-consistent snapshots. Same size as the origin;
            // create is idempotent like the head's.
            let lin = lineage_name(&vol.name);
            self.cp
                .create_lineage_volume(&lin, vol.size)
                .await
                .map_err(cp_status)?;
            let mut ctx: std::collections::HashMap<_, _> = [
                ("mode".to_string(), "thin".to_string()),
                ("size".to_string(), vol.size.to_string()),
                ("lineage".to_string(), lin),
            ]
            .into();
            // Set dials ride along verbatim (already validated, and the
            // strict parse admits only canonical spellings); absent
            // dials emit nothing — the node default must keep applying.
            for key in crate::agentcycle::ReclaimParams::KEYS {
                if let Some(v) = req.parameters.get(key) {
                    ctx.insert(key.to_string(), v.clone());
                }
            }
            ctx
        } else {
            Default::default()
        };
        Ok(Response::new(csi::CreateVolumeResponse {
            volume: Some(csi::Volume {
                volume_id: vol.name,
                capacity_bytes: vol.size as i64,
                volume_context,
                accessible_topology,
                ..Default::default()
            }),
        }))
    }

    async fn delete_volume(
        &self,
        req: Request<csi::DeleteVolumeRequest>,
    ) -> Result<Response<csi::DeleteVolumeResponse>, Status> {
        let req = req.into_inner();
        if req.volume_id.is_empty() {
            return Err(Status::invalid_argument("volume_id is required"));
        }
        // Reference volumes never created anything server-side; the
        // snapshot itself is owned by its VolumeSnapshot object.
        if req.volume_id.starts_with("ro:") {
            return Ok(Response::new(csi::DeleteVolumeResponse {}));
        }
        // Server delete is idempotent (absent → 204). A 409 is either a
        // config-defined volume (operator error) or a source volume whose
        // snapshots still anchor live clones — both are "cannot delete
        // yet", CSI's FAILED_PRECONDITION (the provisioner retries; the
        // clone case clears when the clone volumes are deleted).
        //
        // One deletion shape (D-M15 §3): a thin volume has exactly one
        // sibling, its lineage — deleted first, so a refusal (live
        // clone pins on lineage snapshots) leaves everything intact for
        // the retry. The origin export needs no per-volume cleanup: it
        // lives in the server's registry and its pointer row dies with
        // the lineage's refs object. For raw volumes the sibling is
        // absent and the delete is a 204 no-op.
        match self.cp.delete_volume(&lineage_name(&req.volume_id)).await {
            Ok(()) => {}
            Err(cp::CpError::Conflict(m)) => return Err(Status::failed_precondition(m)),
            Err(e) => return Err(cp_status(e)),
        }
        match self.cp.delete_volume(&req.volume_id).await {
            Ok(()) => Ok(Response::new(csi::DeleteVolumeResponse {})),
            Err(cp::CpError::Conflict(m)) => Err(Status::failed_precondition(m)),
            Err(e) => Err(cp_status(e)),
        }
    }

    async fn validate_volume_capabilities(
        &self,
        req: Request<csi::ValidateVolumeCapabilitiesRequest>,
    ) -> Result<Response<csi::ValidateVolumeCapabilitiesResponse>, Status> {
        use csi::volume_capability::access_mode::Mode;
        let req = req.into_inner();
        // Reference volumes are read-only by construction; real volumes
        // are single-node writers.
        let ro_ref = req.volume_id.starts_with("ro:");
        let supported = req.volume_capabilities.iter().all(|cap| {
            let mode_ok = cap.access_mode.as_ref().is_some_and(|m| {
                if ro_ref {
                    m.mode == Mode::SingleNodeReaderOnly as i32
                        || m.mode == Mode::MultiNodeReaderOnly as i32
                } else {
                    m.mode == Mode::SingleNodeWriter as i32
                }
            });
            mode_ok && capability_shape(Some(cap)).is_some()
        });
        Ok(Response::new(csi::ValidateVolumeCapabilitiesResponse {
            confirmed: supported.then(|| csi::validate_volume_capabilities_response::Confirmed {
                volume_capabilities: req.volume_capabilities,
                ..Default::default()
            }),
            message: if supported {
                String::new()
            } else if ro_ref {
                "snapshot reference volumes support only read-only access modes".into()
            } else {
                "only SINGLE_NODE_WRITER block/mount volumes are supported".into()
            },
        }))
    }

    async fn controller_get_capabilities(
        &self,
        _: Request<csi::ControllerGetCapabilitiesRequest>,
    ) -> Result<Response<csi::ControllerGetCapabilitiesResponse>, Status> {
        use csi::controller_service_capability::{rpc, Rpc, Type};
        let caps = [
            rpc::Type::CreateDeleteVolume,
            rpc::Type::CreateDeleteSnapshot,
        ]
        .into_iter()
        .map(|t| csi::ControllerServiceCapability {
            r#type: Some(Type::Rpc(Rpc { r#type: t as i32 })),
        })
        .collect();
        Ok(Response::new(csi::ControllerGetCapabilitiesResponse {
            capabilities: caps,
        }))
    }

    async fn controller_publish_volume(
        &self,
        _: Request<csi::ControllerPublishVolumeRequest>,
    ) -> Result<Response<csi::ControllerPublishVolumeResponse>, Status> {
        Err(Status::unimplemented("no controller publish step"))
    }

    async fn controller_unpublish_volume(
        &self,
        _: Request<csi::ControllerUnpublishVolumeRequest>,
    ) -> Result<Response<csi::ControllerUnpublishVolumeResponse>, Status> {
        Err(Status::unimplemented("no controller publish step"))
    }

    async fn list_volumes(
        &self,
        _: Request<csi::ListVolumesRequest>,
    ) -> Result<Response<csi::ListVolumesResponse>, Status> {
        Err(Status::unimplemented("LIST_VOLUMES not advertised"))
    }

    async fn get_capacity(
        &self,
        _: Request<csi::GetCapacityRequest>,
    ) -> Result<Response<csi::GetCapacityResponse>, Status> {
        Err(Status::unimplemented("GET_CAPACITY not advertised"))
    }

    /// Server-native snapshot of the source volume (CSI design §3):
    /// crash-consistent for raw-mode volumes — the server has no freeze
    /// hook. `snapshot_id = "<volume>/<name>"`; a duplicate name on the
    /// same volume IS the same snapshot (names are the identity), so a
    /// replayed create returns success per CSI's idempotency contract.
    async fn create_snapshot(
        &self,
        req: Request<csi::CreateSnapshotRequest>,
    ) -> Result<Response<csi::CreateSnapshotResponse>, Status> {
        let req = req.into_inner();
        if req.name.is_empty() || req.source_volume_id.is_empty() {
            return Err(Status::invalid_argument(
                "name and source_volume_id are required",
            ));
        }
        // Thin volumes (marker: the lineage sibling exists) get
        // fs-consistent snapshots through the node's embedded agent
        // cycle (design §3, V3.2 §9): file a snapshot request, wait for
        // its reported capture cut before returning a handle, then let
        // objectize → publish → alias continue behind ready_to_use=false.
        // Requests and readiness checks are both idempotent, so replays
        // at any stage are safe.
        let lin = lineage_name(&req.source_volume_id);
        if let Some(lin_size) = self.cp.volume_size(&lin).await.map_err(cp_status)? {
            let ready = self
                .cp
                .list_snapshots(&lin)
                .await
                .map_err(cp_status)?
                .iter()
                .any(|s| s.name == req.name);
            let status = if !ready {
                self.cp
                    .request_snapshot(&lin, &req.name)
                    .await
                    .map_err(cp_status)?;
                match self.cp.snapshot_request_status(&lin, &req.name).await {
                    Ok(Some(st)) => Some(st),
                    // Without status there is no proof that the capture
                    // happened. Retry rather than exposing a handle whose
                    // point in time could fall outside a caller's quiesce.
                    Ok(None) => None,
                    Err(e) => {
                        tracing::info!(volume = %req.source_volume_id, snapshot = %req.name,
                            error = %e, "thin snapshot requested; awaiting agent capture");
                        None
                    }
                }
            } else {
                // The completed request's retained status carries the
                // original cut's time, which pins replays' creation time.
                self.cp
                    .snapshot_request_status(&lin, &req.name)
                    .await
                    .ok()
                    .flatten()
            };
            // The cut this request was bound to was discarded before it
            // was published, so no snapshot with the point in time a
            // caller may already hold can ever exist. Fail permanently
            // instead of letting the agent serve a different cut.
            if let Some(st) = status.as_ref().filter(|st| st.capture_lost) {
                return Err(Status::failed_precondition(format!(
                    "snapshot {:?} of volume {:?}: its captured point in time was lost before \
                     publication (phase={}); delete and re-create the VolumeSnapshot",
                    req.name, req.source_volume_id, st.phase,
                )));
            }
            if !ready {
                // D-M12: the request carries the agent cycle's reported
                // status. A failing cycle becomes a real gRPC error —
                // the external-snapshotter copies it into the
                // VolumeSnapshot's error field and keeps retrying, so an
                // operator sees "wedged on a freeze" instead of an
                // eternal wait.
                let Some(st) = status.as_ref() else {
                    return Err(Status::unavailable(format!(
                        "snapshot {:?} of volume {:?} awaiting agent capture (phase=unknown)",
                        req.name, req.source_volume_id,
                    )));
                };
                if let Some(error) = st.last_error.as_deref() {
                    return Err(Status::internal(format!(
                        "snapshot {:?} of volume {:?} not ready: agent cycle failing \
                             (phase={}, ops {}/{}, attempt {}): {}",
                        req.name,
                        req.source_volume_id,
                        st.phase,
                        st.ops_done,
                        st.ops_total,
                        st.attempts,
                        error,
                    )));
                }
                if st.capture_time_ms.filter(|&time| time != 0).is_none() {
                    tracing::info!(volume = %req.source_volume_id, snapshot = %req.name,
                        phase = %st.phase, ops_done = st.ops_done, ops_total = st.ops_total,
                        "thin snapshot requested; awaiting agent capture");
                    return Err(Status::unavailable(format!(
                        "snapshot {:?} of volume {:?} awaiting agent capture (phase={})",
                        req.name, req.source_volume_id, st.phase,
                    )));
                }
            }
            // Not-ready responses always carry the bound cut's time (checked
            // above). Only a ready snapshot whose retained status is gone —
            // an API restart after completion — falls back to the clock;
            // the external-snapshotter records creation time from its first
            // successful response, so this only affects a cold replay.
            let creation_time = timestamp_from_millis(
                status
                    .as_ref()
                    .and_then(|st| st.capture_time_ms)
                    .filter(|&time| time != 0)
                    .unwrap_or_else(|| {
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or_default()
                    }),
            );
            return Ok(Response::new(csi::CreateSnapshotResponse {
                snapshot: Some(csi::Snapshot {
                    snapshot_id: format!("{lin}/{}", req.name),
                    source_volume_id: req.source_volume_id,
                    // Restore size: without it the snapshotter records 0
                    // and a restoring PVC's own (possibly smaller) size
                    // reaches the server clone API, which refuses to
                    // shrink below the source — OUT_OF_RANGE forever.
                    size_bytes: lin_size as i64,
                    creation_time: Some(creation_time),
                    ready_to_use: ready,
                    ..Default::default()
                }),
            }));
        }
        match self
            .cp
            .create_snapshot(&req.source_volume_id, &req.name)
            .await
        {
            Ok(s) => {
                tracing::info!(volume = %req.source_volume_id, snapshot = %s.name,
                    commit = %s.commit, "snapshot created")
            }
            // Replay of an identical request: the name already exists on
            // this volume, and the name IS the identity -> same snapshot.
            Err(cp::CpError::Conflict(_)) => {}
            Err(e) => return Err(cp_status(e)),
        }
        // Restore size = source volume size (snapshots are full-volume
        // images); without it restores can under-size (see thin path).
        let size = self
            .cp
            .volume_size(&req.source_volume_id)
            .await
            .map_err(cp_status)?
            .unwrap_or(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Ok(Response::new(csi::CreateSnapshotResponse {
            snapshot: Some(csi::Snapshot {
                snapshot_id: format!("{}/{}", req.source_volume_id, req.name),
                source_volume_id: req.source_volume_id,
                size_bytes: size as i64,
                creation_time: Some(prost_types::Timestamp {
                    seconds: now.as_secs() as i64,
                    nanos: now.subsec_nanos() as i32,
                }),
                ready_to_use: true,
                ..Default::default()
            }),
        }))
    }

    async fn delete_snapshot(
        &self,
        req: Request<csi::DeleteSnapshotRequest>,
    ) -> Result<Response<csi::DeleteSnapshotResponse>, Status> {
        let req = req.into_inner();
        let Some((vol, name)) = req.snapshot_id.split_once('/') else {
            // Not one of ours (or garbage): deleting the nonexistent is
            // success per CSI.
            return Ok(Response::new(csi::DeleteSnapshotResponse {}));
        };
        match self.cp.delete_snapshot(vol, name).await {
            // Absent volume or snapshot: replayed delete, still success.
            Ok(()) | Err(cp::CpError::NotFound(_)) => {
                Ok(Response::new(csi::DeleteSnapshotResponse {}))
            }
            // Pinned snapshots surface as a retryable failure.
            Err(cp::CpError::Conflict(m)) => Err(Status::failed_precondition(m)),
            Err(e) => Err(cp_status(e)),
        }
    }

    async fn list_snapshots(
        &self,
        _: Request<csi::ListSnapshotsRequest>,
    ) -> Result<Response<csi::ListSnapshotsResponse>, Status> {
        Err(Status::unimplemented("LIST_SNAPSHOTS not advertised"))
    }

    async fn controller_expand_volume(
        &self,
        _: Request<csi::ControllerExpandVolumeRequest>,
    ) -> Result<Response<csi::ControllerExpandVolumeResponse>, Status> {
        Err(Status::unimplemented("volume expansion not supported"))
    }

    async fn controller_get_volume(
        &self,
        _: Request<csi::ControllerGetVolumeRequest>,
    ) -> Result<Response<csi::ControllerGetVolumeResponse>, Status> {
        Err(Status::unimplemented("GET_VOLUME not advertised"))
    }

    async fn controller_modify_volume(
        &self,
        _: Request<csi::ControllerModifyVolumeRequest>,
    ) -> Result<Response<csi::ControllerModifyVolumeResponse>, Status> {
        Err(Status::unimplemented("MODIFY_VOLUME not advertised"))
    }
}

// -------------------------------------------------------------------- node

pub struct NodeSvc {
    pub node_id: String,
    pub mechanics: Arc<NodeMechanics>,
}

#[tonic::async_trait]
impl csi::node_server::Node for NodeSvc {
    async fn node_stage_volume(
        &self,
        req: Request<csi::NodeStageVolumeRequest>,
    ) -> Result<Response<csi::NodeStageVolumeResponse>, Status> {
        let req = req.into_inner();
        if req.volume_id.is_empty() || req.staging_target_path.is_empty() {
            return Err(Status::invalid_argument(
                "volume_id and staging_target_path are required",
            ));
        }
        // The same refusal at the point the option is actually dropped:
        // CreateVolume catches new PVCs, this catches a PV that already
        // carries the option (provisioned before the check, or by hand).
        if let Some(why) = guest_discard_refusal(req.volume_capability.as_slice()) {
            return Err(Status::invalid_argument(why));
        }
        let (block, fstype) = capability_shape(req.volume_capability.as_ref())
            .ok_or_else(|| Status::invalid_argument("volume_capability is required"))?;
        // Thin mode arrives via the PV's volume context (set by
        // CreateVolume from the StorageClass `mode` parameter). The
        // lineage name rides along for the embedded agent cycle; PVs
        // from before 6h-2 lack it and simply run without a cycle.
        let thin = match req.volume_context.get("mode").map(String::as_str) {
            Some("thin") => Some(crate::node::ThinParams {
                size_bytes: req
                    .volume_context
                    .get("size")
                    .and_then(|s| s.parse::<u64>().ok())
                    .filter(|s| *s > 0)
                    .ok_or_else(|| {
                        Status::invalid_argument("thin volume context carries no size")
                    })?,
                lineage: req.volume_context.get("lineage").cloned(),
                // §7 dials (task 15i): CreateVolume validated them, but
                // the context is PV data — re-parse strictly and fail
                // the stage on anything unreadable.
                reclaim: crate::agentcycle::ReclaimParams::from_map(&req.volume_context)
                    .map_err(Status::invalid_argument)?,
            }),
            _ => None,
        };
        self.mechanics
            .stage(
                &req.volume_id,
                &req.staging_target_path,
                fstype.as_deref(),
                block,
                thin,
            )
            .await
            .map_err(node_status)?;
        Ok(Response::new(csi::NodeStageVolumeResponse {}))
    }

    async fn node_unstage_volume(
        &self,
        req: Request<csi::NodeUnstageVolumeRequest>,
    ) -> Result<Response<csi::NodeUnstageVolumeResponse>, Status> {
        let req = req.into_inner();
        self.mechanics
            .unstage(&req.volume_id, &req.staging_target_path)
            .await
            .map_err(node_status)?;
        Ok(Response::new(csi::NodeUnstageVolumeResponse {}))
    }

    async fn node_publish_volume(
        &self,
        req: Request<csi::NodePublishVolumeRequest>,
    ) -> Result<Response<csi::NodePublishVolumeResponse>, Status> {
        let req = req.into_inner();
        if req.target_path.is_empty() || req.staging_target_path.is_empty() {
            return Err(Status::invalid_argument(
                "target_path and staging_target_path are required",
            ));
        }
        let (block, _) = capability_shape(req.volume_capability.as_ref())
            .ok_or_else(|| Status::invalid_argument("volume_capability is required"))?;
        self.mechanics
            .publish(
                &req.volume_id,
                &req.staging_target_path,
                &req.target_path,
                block,
                req.readonly,
            )
            .await
            .map_err(node_status)?;
        Ok(Response::new(csi::NodePublishVolumeResponse {}))
    }

    async fn node_unpublish_volume(
        &self,
        req: Request<csi::NodeUnpublishVolumeRequest>,
    ) -> Result<Response<csi::NodeUnpublishVolumeResponse>, Status> {
        let req = req.into_inner();
        self.mechanics
            .unpublish(&req.target_path)
            .await
            .map_err(node_status)?;
        Ok(Response::new(csi::NodeUnpublishVolumeResponse {}))
    }

    async fn node_get_capabilities(
        &self,
        _: Request<csi::NodeGetCapabilitiesRequest>,
    ) -> Result<Response<csi::NodeGetCapabilitiesResponse>, Status> {
        use csi::node_service_capability::{rpc, Rpc, Type};
        Ok(Response::new(csi::NodeGetCapabilitiesResponse {
            capabilities: vec![csi::NodeServiceCapability {
                r#type: Some(Type::Rpc(Rpc {
                    r#type: rpc::Type::StageUnstageVolume as i32,
                })),
            }],
        }))
    }

    async fn node_get_info(
        &self,
        _: Request<csi::NodeGetInfoRequest>,
    ) -> Result<Response<csi::NodeGetInfoResponse>, Status> {
        // Hostname topology lets thin volumes pin to the node holding
        // their cache file (CSI design 6h); raw volumes ignore it.
        Ok(Response::new(csi::NodeGetInfoResponse {
            node_id: self.node_id.clone(),
            accessible_topology: Some(csi::Topology {
                segments: [("kubernetes.io/hostname".to_string(), self.node_id.clone())].into(),
            }),
            ..Default::default()
        }))
    }

    async fn node_get_volume_stats(
        &self,
        _: Request<csi::NodeGetVolumeStatsRequest>,
    ) -> Result<Response<csi::NodeGetVolumeStatsResponse>, Status> {
        Err(Status::unimplemented("GET_VOLUME_STATS not advertised"))
    }

    async fn node_expand_volume(
        &self,
        _: Request<csi::NodeExpandVolumeRequest>,
    ) -> Result<Response<csi::NodeExpandVolumeResponse>, Status> {
        Err(Status::unimplemented("volume expansion not supported"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_rounds_up_and_defaults() {
        assert_eq!(capacity_from_range(None), 1 << 30);
        let r = csi::CapacityRange {
            required_bytes: 4097,
            limit_bytes: 0,
        };
        assert_eq!(capacity_from_range(Some(&r)), 8192);
        let r = csi::CapacityRange {
            required_bytes: 0,
            limit_bytes: 1 << 20,
        };
        assert_eq!(capacity_from_range(Some(&r)), 1 << 20);
    }

    #[test]
    fn capability_shapes() {
        use csi::volume_capability::{AccessType, BlockVolume, MountVolume};
        let block = csi::VolumeCapability {
            access_type: Some(AccessType::Block(BlockVolume {})),
            access_mode: None,
        };
        assert_eq!(capability_shape(Some(&block)), Some((true, None)));
        let mount = csi::VolumeCapability {
            access_type: Some(AccessType::Mount(MountVolume {
                fs_type: "xfs".into(),
                ..Default::default()
            })),
            access_mode: None,
        };
        assert_eq!(
            capability_shape(Some(&mount)),
            Some((false, Some("xfs".into())))
        );
        assert_eq!(capability_shape(None), None);
    }

    /// Invariant B: a StorageClass `mountOptions: [discard]` is inert
    /// here — the node never forwards `mount_flags` — and inert is
    /// exactly why it must be refused. An operator who set it believes
    /// trim is on; on an origin-backed volume a guest trim unmaps
    /// blocks the published generation still serves, which is why the
    /// shim's queue is masked by construction.
    #[test]
    fn guest_discard_mount_options_are_refused_not_dropped() {
        for opt in ["discard", "discard=async"] {
            let why = guest_discard_refusal(&[mount_cap(&["noatime", opt])])
                .expect("silently dropping it misleads the operator");
            assert!(why.contains("Invariant B"), "{why}");
            assert!(why.contains(opt), "{why}");
        }
        // One ENTRY, several options: `mountOptions: ["noatime,discard"]`
        // is legal YAML and the shape a hand-written StorageClass takes,
        // since it is what `mount -o` accepts. Splitting on `=` alone
        // passed it straight through the refusal.
        for entry in ["noatime,discard", "discard=async,noatime", "ro, discard"] {
            let why = guest_discard_refusal(&[mount_cap(&[entry])])
                .unwrap_or_else(|| panic!("comma-joined options are options: {entry:?}"));
            assert!(why.contains(entry), "{why}");
        }
        // `nodiscard` is the posture this driver already enforces, and
        // options that merely contain the word are not it — including
        // inside a comma-joined entry.
        assert_eq!(
            guest_discard_refusal(&[mount_cap(&[
                "nodiscard",
                "nodiscard=1",
                "noatime,nodiscard",
                "data=ordered"
            ])]),
            None,
            "the default posture is fine"
        );
    }

    fn mount_cap(opts: &[&str]) -> csi::VolumeCapability {
        use csi::volume_capability::{AccessType, MountVolume};
        csi::VolumeCapability {
            access_type: Some(AccessType::Mount(MountVolume {
                fs_type: "ext4".into(),
                mount_flags: opts.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            })),
            access_mode: None,
        }
    }

    /// The refusal is wired into CreateVolume, not merely available to
    /// it. The controller here talks to a closed port, so any request
    /// that reached the control plane would come back INTERNAL — the
    /// INVALID_ARGUMENT proves the check ran first, at the one moment an
    /// operator is watching a PVC.
    #[tokio::test]
    async fn create_volume_refuses_a_discard_mount_option() {
        use csi::controller_server::Controller;
        let svc = ControllerSvc {
            cp: cp::Client::new("http://127.0.0.1:9", None),
        };
        let err = svc
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "pvc-trim".into(),
                volume_capabilities: vec![mount_cap(&["noatime,discard"])],
                ..Default::default()
            }))
            .await
            .expect_err("a volume that cannot honor its mountOptions must not be created");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("Invariant B"), "{err}");
    }

    /// And into NodeStageVolume, which is where a PV provisioned before
    /// the check — or by hand — arrives. Nothing may be attached first:
    /// the refusal is permanent (kubelet will retry it forever), so a
    /// stage that got as far as `nbd-client` would burn a device per
    /// retry.
    #[tokio::test]
    async fn node_stage_refuses_a_discard_mount_option_before_attaching() {
        use crate::node::{NbdEndpoint, NodeConfig};
        use crate::runner::mock::MockRunner;
        use csi::node_server::Node;

        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(MockRunner::default());
        let m = crate::node::NodeMechanics::new(
            NodeConfig {
                endpoint: NbdEndpoint::Tcp {
                    host: "skynbd.example".into(),
                    port: 10809,
                    tls_hostname: None,
                    creds_dir: None,
                },
                sysfs_block: dir.path().join("sys-block"),
                state_dir: dir.path().join("state"),
                default_fstype: "ext4".into(),
                cache_dir: None,
                cache_bytes: 0,
                chunk_bytes: 64 << 10,
                read_cache_dir: None,
                read_cache_bytes: 0,
            },
            runner.clone() as Arc<dyn crate::runner::Runner>,
            None,
            None,
        );
        let svc = NodeSvc {
            node_id: "node-1".into(),
            mechanics: Arc::new(m),
        };
        let err = svc
            .node_stage_volume(Request::new(csi::NodeStageVolumeRequest {
                volume_id: "pvc-trim".into(),
                staging_target_path: dir.path().join("stage").display().to_string(),
                volume_capability: Some(mount_cap(&["discard"])),
                ..Default::default()
            }))
            .await
            .expect_err("an existing PV carrying the option is unstageable — intentionally");
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("Invariant B"), "{err}");
        assert!(
            runner.calls().is_empty(),
            "nothing may be attached for a stage that can only fail: {:?}",
            runner.calls()
        );
    }

    fn ro_cap() -> csi::VolumeCapability {
        use csi::volume_capability::{access_mode, AccessMode, AccessType, MountVolume};
        csi::VolumeCapability {
            access_type: Some(AccessType::Mount(MountVolume::default())),
            access_mode: Some(AccessMode {
                mode: access_mode::Mode::MultiNodeReaderOnly as i32,
            }),
        }
    }

    fn snap_source(id: &str) -> Option<csi::VolumeContentSource> {
        use csi::volume_content_source::{SnapshotSource, Type};
        Some(csi::VolumeContentSource {
            r#type: Some(Type::Snapshot(SnapshotSource {
                snapshot_id: id.into(),
            })),
        })
    }

    /// RO snapshot restores never touch the control plane: the cp base
    /// here is a closed port, so any request would error loudly.
    #[tokio::test]
    async fn ro_snapshot_restore_is_a_reference_volume_without_server_calls() {
        use csi::controller_server::Controller;
        let svc = ControllerSvc {
            cp: cp::Client::new("http://127.0.0.1:9", None),
        };

        let resp = svc
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "pvc-ro".into(),
                volume_capabilities: vec![ro_cap()],
                volume_content_source: snap_source("vol1/snap-a"),
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        let vol = resp.volume.unwrap();
        assert_eq!(vol.volume_id, "ro:vol1/snap/snap-a");
        assert_eq!(vol.capacity_bytes, 0);
        // The spec (and external-provisioner) require the content source
        // echoed back.
        assert_eq!(vol.content_source, snap_source("vol1/snap-a"));

        // Deleting a reference volume is a no-op, also server-free.
        svc.delete_volume(Request::new(csi::DeleteVolumeRequest {
            volume_id: vol.volume_id.clone(),
            ..Default::default()
        }))
        .await
        .unwrap();

        // Writable restore is a *clone* — a real server call, so against
        // this closed port it must fail loudly (transport → INTERNAL),
        // proving the RO path above really was server-free.
        let mut rw = ro_cap();
        rw.access_mode.as_mut().unwrap().mode =
            csi::volume_capability::access_mode::Mode::SingleNodeWriter as i32;
        let err = svc
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "pvc-rw".into(),
                volume_capabilities: vec![rw],
                volume_content_source: snap_source("vol1/snap-a"),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);

        // Garbage snapshot id (no separator): NOT_FOUND.
        let err = svc
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "pvc-x".into(),
                volume_capabilities: vec![ro_cap()],
                volume_content_source: snap_source("nonsense"),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);

        // Validate: RO modes confirm on reference volumes, RW does not.
        let ok = svc
            .validate_volume_capabilities(Request::new(csi::ValidateVolumeCapabilitiesRequest {
                volume_id: "ro:vol1/snap/snap-a".into(),
                volume_capabilities: vec![ro_cap()],
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(ok.confirmed.is_some());
        let bad = svc
            .validate_volume_capabilities(Request::new(csi::ValidateVolumeCapabilitiesRequest {
                volume_id: "ro:vol1/snap/snap-a".into(),
                volume_capabilities: vec![{
                    let mut c = ro_cap();
                    c.access_mode.as_mut().unwrap().mode =
                        csi::volume_capability::access_mode::Mode::SingleNodeWriter as i32;
                    c
                }],
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(bad.confirmed.is_none());
    }

    /// Stub control plane speaking just enough of the snapshot + clone
    /// APIs: duplicate names 409 (create is idempotent for identical
    /// replays), unknown volume/snapshot 404, clone size below the
    /// 64 MiB source 400, clone size 0 inherits.
    /// Handles returned beside the base URL so tests can observe what
    /// the controller filed (snapshot requests) without extra routes.
    type StubRequests =
        std::sync::Arc<std::sync::Mutex<std::collections::HashSet<(String, String)>>>;
    /// Per-request D-M12 status the stub serves on
    /// `GET snapshot-requests/{name}`; tests poke it directly instead of
    /// modeling the agent's cycle-status reports.
    type StubStatuses = std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<(String, String), serde_json::Value>>,
    >;

    async fn spawn_stub() -> (String, StubRequests, StubStatuses) {
        use axum::extract::Path;
        use axum::http::StatusCode;
        use axum::routing::{delete, get, post};
        use std::collections::{HashMap, HashSet};
        use std::sync::{Arc as StdArc, Mutex};

        const SRC_SIZE: u64 = 64 << 20;
        let snaps: StdArc<Mutex<HashSet<(String, String)>>> = Default::default();
        let s1 = snaps.clone();
        let s2 = snaps.clone();
        let s3 = snaps.clone();
        let vols: StdArc<Mutex<HashMap<String, u64>>> = Default::default();
        let v_list = vols.clone();
        let v_del = vols.clone();
        let requests: StubRequests = Default::default();
        let r1 = requests.clone();
        let r2 = requests.clone();
        let statuses: StubStatuses = Default::default();
        let st1 = statuses.clone();
        let app = axum::Router::new()
            .route(
                "/v1/volumes",
                post(move |body: axum::Json<serde_json::Value>| {
                    let vols = vols.clone();
                    async move {
                        let name = body["name"].as_str().unwrap().to_string();
                        let mut size = body["size"].as_u64().unwrap_or(0);
                        if let Some(from) = body["from"].as_str() {
                            if from.starts_with("missing/") {
                                return (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({})));
                            }
                            if size == 0 {
                                size = SRC_SIZE;
                            } else if size < SRC_SIZE {
                                return (
                                    StatusCode::BAD_REQUEST,
                                    axum::Json(serde_json::json!({})),
                                );
                            }
                        }
                        let mut vols = vols.lock().unwrap();
                        let (code, size) = match vols.get(&name) {
                            Some(&existing) if existing == size => (StatusCode::OK, existing),
                            Some(_) => {
                                return (StatusCode::CONFLICT, axum::Json(serde_json::json!({})))
                            }
                            None => {
                                vols.insert(name.clone(), size);
                                (StatusCode::CREATED, size)
                            }
                        };
                        (
                            code,
                            axum::Json(serde_json::json!({"name": name, "size": size})),
                        )
                    }
                }),
            )
            .route(
                "/v1/volumes/{vol}/snapshots",
                post(
                    move |Path(vol): Path<String>, body: axum::Json<serde_json::Value>| {
                        let snaps = s1.clone();
                        async move {
                            if vol == "missing" {
                                return (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({})));
                            }
                            let name = body["name"].as_str().unwrap().to_string();
                            if !snaps.lock().unwrap().insert((vol, name.clone())) {
                                return (StatusCode::CONFLICT, axum::Json(serde_json::json!({})));
                            }
                            (
                                StatusCode::CREATED,
                                axum::Json(serde_json::json!({"name": name, "commit": "aa"})),
                            )
                        }
                    },
                ),
            )
            .route(
                "/v1/volumes/{vol}/snapshots/{name}",
                delete(move |Path((vol, name)): Path<(String, String)>| {
                    let snaps = s2.clone();
                    async move {
                        if snaps.lock().unwrap().remove(&(vol, name)) {
                            StatusCode::NO_CONTENT
                        } else {
                            StatusCode::NOT_FOUND
                        }
                    }
                }),
            );
        let app = app
            .route(
                "/v1/volumes",
                get(move || {
                    let vols = v_list.clone();
                    async move {
                        let vols = vols.lock().unwrap();
                        let list: Vec<serde_json::Value> = vols
                            .iter()
                            .map(|(n, s)| serde_json::json!({"name": n, "size": s}))
                            .collect();
                        axum::Json(serde_json::json!(list))
                    }
                }),
            )
            .route(
                "/v1/volumes/{vol}",
                delete(move |Path(vol): Path<String>| {
                    let vols = v_del.clone();
                    async move {
                        // Absent → 204, like the real server.
                        vols.lock().unwrap().remove(&vol);
                        StatusCode::NO_CONTENT
                    }
                }),
            )
            .route(
                "/v1/volumes/{vol}/snapshots",
                get(move |Path(vol): Path<String>| {
                    let snaps = s3.clone();
                    async move {
                        let list: Vec<serde_json::Value> = snaps
                            .lock()
                            .unwrap()
                            .iter()
                            .filter(|(v, _)| *v == vol)
                            .map(|(_, n)| serde_json::json!({"name": n, "commit": "aa"}))
                            .collect();
                        axum::Json(serde_json::json!(list))
                    }
                }),
            )
            .route(
                "/v1/volumes/{vol}/snapshot-requests",
                post(
                    move |Path(vol): Path<String>, body: axum::Json<serde_json::Value>| {
                        let requests = r1.clone();
                        async move {
                            let name = body["name"].as_str().unwrap().to_string();
                            requests.lock().unwrap().insert((vol, name));
                            StatusCode::ACCEPTED
                        }
                    },
                ),
            )
            .route(
                "/v1/volumes/{vol}/snapshot-requests/{name}",
                get(move |Path((vol, name)): Path<(String, String)>| {
                    let requests = r2.clone();
                    let statuses = st1.clone();
                    async move {
                        let key = (vol, name);
                        if !requests.lock().unwrap().contains(&key) {
                            return (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({})));
                        }
                        let st = statuses.lock().unwrap().get(&key).cloned().unwrap_or(
                            serde_json::json!({
                                "phase": "pending", "ops_done": 0, "ops_total": 0,
                                "attempts": 0, "last_error": null, "capture_time_ms": null
                            }),
                        );
                        (StatusCode::OK, axum::Json(st))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, requests, statuses)
    }

    /// Thin snapshots (6h-2): CreateSnapshot on a volume with a lineage
    /// sibling waits for the (simulated) agent's capture cut before
    /// returning a not-ready handle.
    #[tokio::test(flavor = "multi_thread")]
    async fn thin_snapshot_routes_through_lineage_requests() {
        use csi::controller_server::Controller;
        let (base, requests, statuses) = spawn_stub().await;
        let svc = ControllerSvc {
            cp: cp::Client::new(&base, None),
        };
        let node_topo = csi::Topology {
            segments: [("kubernetes.io/hostname".to_string(), "node-a".to_string())].into(),
        };
        svc.create_volume(Request::new(csi::CreateVolumeRequest {
            name: "pvc-t".into(),
            volume_capabilities: vec![rw_cap()],
            parameters: [("mode".to_string(), "thin".to_string())].into(),
            accessibility_requirements: Some(csi::TopologyRequirement {
                requisite: vec![node_topo.clone()],
                preferred: vec![node_topo],
            }),
            ..Default::default()
        }))
        .await
        .unwrap();

        let req = || {
            Request::new(csi::CreateSnapshotRequest {
                source_volume_id: "pvc-t".into(),
                name: "s1".into(),
                ..Default::default()
            })
        };
        // A pre-capture request cannot expose a handle: callers' external
        // application quiesce brackets the first successful response.
        let err = svc.create_snapshot(req()).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unavailable);
        assert!(err.message().contains("phase=pending"));
        assert!(requests
            .lock()
            .unwrap()
            .contains(&("pvc-t-lin".to_string(), "s1".to_string())));
        statuses.lock().unwrap().insert(
            ("pvc-t-lin".into(), "s1".into()),
            serde_json::json!({
                "phase": "capturing", "ops_done": 0, "ops_total": 0,
                "attempts": 0, "last_error": null, "capture_time_ms": null
            }),
        );
        let err = svc.create_snapshot(req()).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unavailable);
        assert!(err.message().contains("phase=capturing"));

        // Once the agent reports the cut, a handle is safe even while
        // objectization and publication remain pending.
        const CAPTURE_TIME_MS: u64 = 1_700_000_123_456;
        statuses.lock().unwrap().insert(
            ("pvc-t-lin".into(), "s1".into()),
            serde_json::json!({
                "phase": "captured", "ops_done": 0, "ops_total": 0,
                "attempts": 0, "last_error": null,
                "capture_time_ms": CAPTURE_TIME_MS
            }),
        );
        let snap = svc.create_snapshot(req()).await.unwrap().into_inner();
        let snap = snap.snapshot.unwrap();
        assert!(!snap.ready_to_use);
        assert_eq!(snap.snapshot_id, "pvc-t-lin/s1");
        assert_eq!(
            snap.creation_time,
            Some(prost_types::Timestamp {
                seconds: 1_700_000_123,
                nanos: 456_000_000,
            })
        );
        // Restore size travels with the snapshot so restoring PVCs are
        // sized at least at the source volume (default 1 GiB here).
        assert_eq!(snap.size_bytes, 1 << 30);

        // Replay after the cut: same answer, still idempotent.
        let again = svc.create_snapshot(req()).await.unwrap().into_inner();
        let again = again.snapshot.unwrap();
        assert!(!again.ready_to_use);
        assert_eq!(again.creation_time, snap.creation_time);

        // "Agent publishes + aliases": the named snapshot appears in the
        // lineage volume; the next CreateSnapshot reports ready.
        svc.cp.create_snapshot("pvc-t-lin", "s1").await.unwrap();
        let ready = svc.create_snapshot(req()).await.unwrap().into_inner();
        let ready = ready.snapshot.unwrap();
        assert!(ready.ready_to_use);
        assert_eq!(ready.snapshot_id, "pvc-t-lin/s1");
        assert_eq!(ready.creation_time, snap.creation_time);

        // DeleteVolume removes head AND lineage sibling.
        svc.delete_volume(Request::new(csi::DeleteVolumeRequest {
            volume_id: "pvc-t".into(),
            ..Default::default()
        }))
        .await
        .unwrap();
        assert!(svc.cp.volume_size("pvc-t").await.unwrap().is_none());
        assert!(svc.cp.volume_size("pvc-t-lin").await.unwrap().is_none());
    }

    /// D-M12: a failing agent cycle surfaces as a real gRPC error with
    /// phase/attempts/error in the message (the external-snapshotter
    /// copies it into the VolumeSnapshot's error field), while a healthy
    /// post-capture cycle stays a quiet not-ready — and readiness always
    /// wins over a stale error.
    #[tokio::test(flavor = "multi_thread")]
    async fn thin_snapshot_surfaces_cycle_errors() {
        use csi::controller_server::Controller;
        let (base, _requests, statuses) = spawn_stub().await;
        let svc = ControllerSvc {
            cp: cp::Client::new(&base, None),
        };
        let node_topo = csi::Topology {
            segments: [("kubernetes.io/hostname".to_string(), "node-a".to_string())].into(),
        };
        svc.create_volume(Request::new(csi::CreateVolumeRequest {
            name: "pvc-e".into(),
            volume_capabilities: vec![rw_cap()],
            parameters: [("mode".to_string(), "thin".to_string())].into(),
            accessibility_requirements: Some(csi::TopologyRequirement {
                requisite: vec![node_topo.clone()],
                preferred: vec![node_topo],
            }),
            ..Default::default()
        }))
        .await
        .unwrap();

        let req = || {
            Request::new(csi::CreateSnapshotRequest {
                source_volume_id: "pvc-e".into(),
                name: "s1".into(),
                ..Default::default()
            })
        };
        // Pre-capture: file the request but retry without a handle.
        let err = svc.create_snapshot(req()).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unavailable);

        // The agent reports a failing cycle: the next poll is an error
        // carrying the reason.
        statuses.lock().unwrap().insert(
            ("pvc-e-lin".into(), "s1".into()),
            serde_json::json!({
                "phase": "capturing", "ops_done": 0, "ops_total": 0,
                "attempts": 3, "last_error": "freeze: device busy"
            }),
        );
        let err = svc.create_snapshot(req()).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
        for needle in ["freeze: device busy", "capturing", "attempt 3"] {
            assert!(
                err.message().contains(needle),
                "error message missing {needle:?}: {}",
                err.message()
            );
        }

        // Recovered mid-flight (progress, no error): quiet not-ready.
        statuses.lock().unwrap().insert(
            ("pvc-e-lin".into(), "s1".into()),
            serde_json::json!({
                "phase": "backfilling", "ops_done": 5, "ops_total": 10,
                "attempts": 0, "last_error": null,
                "capture_time_ms": 1_700_000_000_000u64
            }),
        );
        let snap = svc.create_snapshot(req()).await.unwrap().into_inner();
        assert!(!snap.snapshot.unwrap().ready_to_use);

        // Published: readiness short-circuits even a stale error status.
        statuses.lock().unwrap().insert(
            ("pvc-e-lin".into(), "s1".into()),
            serde_json::json!({
                "phase": "backfilling", "ops_done": 5, "ops_total": 10,
                "attempts": 9, "last_error": "stale"
            }),
        );
        svc.cp.create_snapshot("pvc-e-lin", "s1").await.unwrap();
        let ready = svc.create_snapshot(req()).await.unwrap().into_inner();
        assert!(ready.snapshot.unwrap().ready_to_use);
    }

    /// A request whose captured cut was discarded unpublished fails
    /// permanently: retrying could only ever serve a later cut than the
    /// one the caller's quiesce bracketed.
    #[tokio::test(flavor = "multi_thread")]
    async fn thin_snapshot_with_a_lost_capture_fails_permanently() {
        use csi::controller_server::Controller;
        let (base, _requests, statuses) = spawn_stub().await;
        let svc = ControllerSvc {
            cp: cp::Client::new(&base, None),
        };
        let node_topo = csi::Topology {
            segments: [("kubernetes.io/hostname".to_string(), "node-a".to_string())].into(),
        };
        svc.create_volume(Request::new(csi::CreateVolumeRequest {
            name: "pvc-l".into(),
            volume_capabilities: vec![rw_cap()],
            parameters: [("mode".to_string(), "thin".to_string())].into(),
            accessibility_requirements: Some(csi::TopologyRequirement {
                requisite: vec![node_topo.clone()],
                preferred: vec![node_topo],
            }),
            ..Default::default()
        }))
        .await
        .unwrap();
        statuses.lock().unwrap().insert(
            ("pvc-l-lin".into(), "s1".into()),
            serde_json::json!({
                "phase": "visible", "ops_done": 0, "ops_total": 0,
                "attempts": 0, "last_error": null,
                "capture_time_ms": 1_700_000_000_000u64, "capture_lost": true
            }),
        );
        let err = svc
            .create_snapshot(Request::new(csi::CreateSnapshotRequest {
                source_volume_id: "pvc-l".into(),
                name: "s1".into(),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
        assert!(err.message().contains("lost"), "{}", err.message());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn snapshot_create_is_idempotent_and_delete_tolerates_absent() {
        use csi::controller_server::Controller;
        let (base, _requests, _statuses) = spawn_stub().await;
        let svc = ControllerSvc {
            cp: cp::Client::new(&base, None),
        };

        let req = || {
            Request::new(csi::CreateSnapshotRequest {
                source_volume_id: "vol1".into(),
                name: "snap-a".into(),
                ..Default::default()
            })
        };
        let snap = svc.create_snapshot(req()).await.unwrap().into_inner();
        let snap = snap.snapshot.unwrap();
        assert_eq!(snap.snapshot_id, "vol1/snap-a");
        assert_eq!(snap.source_volume_id, "vol1");
        assert!(snap.ready_to_use);
        assert!(snap.creation_time.is_some());

        // Replay (server 409): still success, same id.
        let again = svc.create_snapshot(req()).await.unwrap().into_inner();
        assert_eq!(again.snapshot.unwrap().snapshot_id, "vol1/snap-a");

        // Unknown source volume: NOT_FOUND surfaces.
        let missing = svc
            .create_snapshot(Request::new(csi::CreateSnapshotRequest {
                source_volume_id: "missing".into(),
                name: "s".into(),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(missing.code(), tonic::Code::NotFound);

        // Delete, then replay: both succeed.
        let del = |id: &str| {
            Request::new(csi::DeleteSnapshotRequest {
                snapshot_id: id.into(),
                ..Default::default()
            })
        };
        svc.delete_snapshot(del("vol1/snap-a")).await.unwrap();
        svc.delete_snapshot(del("vol1/snap-a")).await.unwrap();
        // Garbage id (no separator): success, nothing to do.
        svc.delete_snapshot(del("garbage")).await.unwrap();
    }

    fn rw_cap() -> csi::VolumeCapability {
        use csi::volume_capability::{access_mode, AccessMode, AccessType, MountVolume};
        csi::VolumeCapability {
            access_type: Some(AccessType::Mount(MountVolume::default())),
            access_mode: Some(AccessMode {
                mode: access_mode::Mode::SingleNodeWriter as i32,
            }),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rw_snapshot_restore_is_a_server_fast_clone() {
        use csi::controller_server::Controller;
        let (base, _requests, _statuses) = spawn_stub().await;
        let svc = ControllerSvc {
            cp: cp::Client::new(&base, None),
        };

        // No capacity range: the server inherits the source size, and the
        // response must carry it (plus the echoed content source) so the
        // PV records real capacity.
        let req = || {
            Request::new(csi::CreateVolumeRequest {
                name: "pvc-clone".into(),
                volume_capabilities: vec![rw_cap()],
                volume_content_source: snap_source("vol1/snap-a"),
                ..Default::default()
            })
        };
        let vol = svc.create_volume(req()).await.unwrap().into_inner();
        let vol = vol.volume.unwrap();
        assert_eq!(vol.volume_id, "pvc-clone");
        assert_eq!(vol.capacity_bytes, 64 << 20);
        assert_eq!(vol.content_source, snap_source("vol1/snap-a"));

        // Identical replay: idempotent success (server 200), same volume.
        let again = svc.create_volume(req()).await.unwrap().into_inner();
        assert_eq!(again.volume.unwrap().volume_id, "pvc-clone");

        // A grown clone keeps the requested capacity.
        let grown = svc
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "pvc-clone-big".into(),
                capacity_range: Some(csi::CapacityRange {
                    required_bytes: 128 << 20,
                    limit_bytes: 0,
                }),
                volume_capabilities: vec![rw_cap()],
                volume_content_source: snap_source("vol1/snap-a"),
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(grown.volume.unwrap().capacity_bytes, 128 << 20);

        // Below the source size: server 400 → OUT_OF_RANGE.
        let err = svc
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "pvc-clone-small".into(),
                capacity_range: Some(csi::CapacityRange {
                    required_bytes: 4096,
                    limit_bytes: 0,
                }),
                volume_capabilities: vec![rw_cap()],
                volume_content_source: snap_source("vol1/snap-a"),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::OutOfRange);

        // Source snapshot gone: NOT_FOUND (the provisioner surfaces it).
        let err = svc
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "pvc-orphan".into(),
                volume_capabilities: vec![rw_cap()],
                volume_content_source: snap_source("missing/snap"),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);

        // The clone is a real volume: SINGLE_NODE_WRITER validates.
        let ok = svc
            .validate_volume_capabilities(Request::new(csi::ValidateVolumeCapabilitiesRequest {
                volume_id: "pvc-clone".into(),
                volume_capabilities: vec![rw_cap()],
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(ok.confirmed.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn thin_mode_rounds_size_and_carries_volume_context() {
        use csi::controller_server::Controller;
        let (base, _requests, _statuses) = spawn_stub().await;
        let svc = ControllerSvc {
            cp: cp::Client::new(&base, None),
        };
        let params = std::collections::HashMap::from([("mode".to_string(), "thin".to_string())]);
        let node_topo = csi::Topology {
            segments: [("kubernetes.io/hostname".to_string(), "node-a".to_string())].into(),
        };
        let reqs = Some(csi::TopologyRequirement {
            requisite: vec![node_topo.clone()],
            preferred: vec![node_topo.clone()],
        });

        // 100 MiB + 1 byte rounds up to whole MiBs (chunk-agnostic).
        let vol = svc
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "pvc-thin".into(),
                capacity_range: Some(csi::CapacityRange {
                    required_bytes: (100 << 20) + 1,
                    limit_bytes: 0,
                }),
                volume_capabilities: vec![rw_cap()],
                parameters: params.clone(),
                accessibility_requirements: reqs.clone(),
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner()
            .volume
            .unwrap();
        assert_eq!(vol.capacity_bytes, 101 << 20);
        assert_eq!(vol.volume_context.get("mode").unwrap(), "thin");
        assert_eq!(
            vol.volume_context.get("size").unwrap(),
            &(101u64 << 20).to_string()
        );
        // The lineage sibling was created server-side and rides along in
        // the context for the node's embedded agent.
        assert_eq!(vol.volume_context.get("lineage").unwrap(), "pvc-thin-lin");
        assert!(svc.cp.volume_size("pvc-thin-lin").await.unwrap().is_some());
        // Node-sticky: the PV pins to the selected node (the thin
        // metadata will live in that node's cache file).
        assert_eq!(vol.accessible_topology, vec![node_topo]);

        // Thin without topology (Immediate binding) → INVALID_ARGUMENT
        // pointing at WaitForFirstConsumer.
        let err = svc
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "pvc-thin-imm".into(),
                volume_capabilities: vec![rw_cap()],
                parameters: params.clone(),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);

        // Raw volumes carry no context.
        let raw = svc
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "pvc-raw".into(),
                volume_capabilities: vec![rw_cap()],
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner()
            .volume
            .unwrap();
        assert!(raw.volume_context.is_empty());

        // Unknown mode → INVALID_ARGUMENT.
        let err = svc
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "pvc-x".into(),
                volume_capabilities: vec![rw_cap()],
                parameters: std::collections::HashMap::from([(
                    "mode".to_string(),
                    "fancy".to_string(),
                )]),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);

        // Thin + snapshot source → INVALID_ARGUMENT (V3.2 §15.7: the
        // thin head is pool-physical; lineage restores land in 6h-2).
        let err = svc
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "pvc-thin-restore".into(),
                volume_capabilities: vec![rw_cap()],
                parameters: params,
                volume_content_source: snap_source("vol1/snap-a"),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    /// Task 15i: the §7 reclaim dials ride the StorageClass parameters
    /// into the volume context — set dials verbatim, absent dials
    /// absent (the node default must keep applying) — and validation
    /// is strict: unknown values and dials on raw volumes are
    /// INVALID_ARGUMENT at provisioning time.
    #[tokio::test(flavor = "multi_thread")]
    async fn thin_mode_reclaim_dials_ride_the_volume_context() {
        use csi::controller_server::Controller;
        let (base, _requests, _statuses) = spawn_stub().await;
        let svc = ControllerSvc {
            cp: cp::Client::new(&base, None),
        };
        let node_topo = csi::Topology {
            segments: [("kubernetes.io/hostname".to_string(), "node-a".to_string())].into(),
        };
        let reqs = Some(csi::TopologyRequirement {
            requisite: vec![node_topo.clone()],
            preferred: vec![node_topo.clone()],
        });
        let thin_req = |name: &str, extra: &[(&str, &str)]| {
            let mut parameters =
                std::collections::HashMap::from([("mode".to_string(), "thin".to_string())]);
            parameters.extend(extra.iter().map(|&(k, v)| (k.to_string(), v.to_string())));
            Request::new(csi::CreateVolumeRequest {
                name: name.into(),
                volume_capabilities: vec![rw_cap()],
                parameters,
                accessibility_requirements: reqs.clone(),
                ..Default::default()
            })
        };

        // All four dials set: each lands in the context verbatim.
        let dials = [
            ("reclaimStrategy", "swing"),
            ("reclaimTiming", "lazy"),
            ("reclaimRangeThreshold", "512"),
            ("swingEveryNAdvances", "4"),
        ];
        let vol = svc
            .create_volume(thin_req("pvc-dials", &dials))
            .await
            .unwrap()
            .into_inner()
            .volume
            .unwrap();
        for (k, v) in dials {
            assert_eq!(
                vol.volume_context.get(k).map(String::as_str),
                Some(v),
                "{k}"
            );
        }
        // And the stage parse accepts exactly what CreateVolume emitted.
        let parsed = crate::agentcycle::ReclaimParams::from_map(&vol.volume_context).unwrap();
        assert_eq!(parsed.strategy, Some(skynbd_agent::ReclaimStrategy::Swing));
        assert_eq!(parsed.timing, Some(skynbd_agent::ReclaimTiming::Lazy));
        assert_eq!(parsed.range_threshold, Some(512));
        assert_eq!(parsed.swing_every_n, Some(4));

        // No dials: none emitted (defaults stay node-level).
        let vol = svc
            .create_volume(thin_req("pvc-nodials", &[]))
            .await
            .unwrap()
            .into_inner()
            .volume
            .unwrap();
        for (k, _) in dials {
            assert!(!vol.volume_context.contains_key(k), "{k} leaked a default");
        }

        // Invalid dial value: INVALID_ARGUMENT naming the value.
        let err = svc
            .create_volume(thin_req("pvc-bad", &[("reclaimStrategy", "aggressive")]))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("aggressive"), "{}", err.message());

        // Dials on a raw volume: rejected loudly, not silently ignored.
        let err = svc
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "pvc-raw-dials".into(),
                volume_capabilities: vec![rw_cap()],
                parameters: std::collections::HashMap::from([(
                    "reclaimTiming".to_string(),
                    "lazy".to_string(),
                )]),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("mode: thin"), "{}", err.message());

        // `swingEveryNAdvances: 0` reads as "swing every advance", which
        // is one keystroke from the unset spelling that means "never".
        let err = svc
            .create_volume(thin_req("pvc-zero-aging", &[("swingEveryNAdvances", "0")]))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(
            err.message().contains("reclaimStrategy: swing"),
            "{}",
            err.message()
        );

        // A thin BLOCK volume starts no cycle, so the dials are inert
        // there — rejected the same way a raw volume's are.
        let err = svc
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "pvc-thin-block-dials".into(),
                volume_capabilities: vec![csi::VolumeCapability {
                    access_type: Some(csi::volume_capability::AccessType::Block(
                        csi::volume_capability::BlockVolume {},
                    )),
                    access_mode: None,
                }],
                parameters: std::collections::HashMap::from([
                    ("mode".to_string(), "thin".to_string()),
                    ("reclaimTiming".to_string(), "lazy".to_string()),
                ]),
                accessibility_requirements: reqs.clone(),
                ..Default::default()
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(
            err.message().contains("filesystem volume"),
            "{}",
            err.message()
        );
        // A thin filesystem volume with the same dial still provisions.
        svc.create_volume(thin_req(
            "pvc-thin-mount-dials",
            &[("reclaimTiming", "lazy")],
        ))
        .await
        .unwrap();
    }
}
