-- New degraded rows must carry the phrases that caused the decision. Historical
-- rows are intentionally not backfilled, so validation remains deferred.
alter table intelligence_detection_records
  add constraint intelligence_detection_records_degraded_phrases_ck
  check (
    not degraded
    or (matched_phrases is not null and cardinality(matched_phrases) > 0)
  ) not valid;
