# Design §5.1 "Reads leave the worker" (zfs-topology a45338f), read before M1.3j

1. A hole as written. Between the log's start moving and the index
   letting go, a reader that found a record below the new start reads
   it through `Log::read_payload`, which refuses a record below the
   floor (`BadPosition`) and a segment no longer in `live`
   (`MissingSegment`); `Log::reclaim` changes both when the start
   moves. The store fails the volume on a failed read. So "only two
   points" does not hold for the log as built: either the reader's
   read of the log does not consult the floor and the live table (it
   resolves file and offset while it holds the index lock), or the
   log forgets the segments only at the second point.
2. The wait must be for the readers under way when it began, not for
   "no reader". Reads follow each other without a gap under load; a
   wait for a count of zero can starve, and the worker waiting is
   every write and flush waiting.
3. "A write a read has shown stays applied" holds, and I see no way
   it fails with several readers for reads that do not overlap in
   time: per block the index only moves from a record to a newer
   record or to the home image that has that record applied. It needs
   the lookup of a read's whole range under one hold and the insert
   of a write's whole range under one hold.
4. The failure mark: a reader that finds a read error must not send
   its failed completion before the worker has marked the log, since
   the mark in the log is the worker's.
5. Discards are not in the index and change nothing a read sees; no
   hole there.
