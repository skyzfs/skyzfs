
// ---- Fourth look at #147 (scratch, not part of the pull request) ----
impl FakeDaemonState {
    /// Whether a request of `name` pends, not withdrawn, before its
    /// deadline: one that may still be published.
    pub fn fl147_pends(&self, name: &str, now: u64) -> bool {
        self.pending
            .iter()
            .any(|p| p.request.name == name && !p.withdrawn && now < p.deadline)
    }
}
