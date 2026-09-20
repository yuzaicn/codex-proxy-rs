-- Manual rollback for 0012_detection_fingerprint.sql.
-- Stop the detection worker and revert the application code before running.
begin;

alter table intelligence_detection_records
  drop constraint intelligence_detection_records_fingerprint_degraded_ck,
  drop constraint intelligence_detection_records_legacy_pair_ck,
  drop constraint intelligence_detection_records_detector_ck,
  drop constraint intelligence_detection_records_verdict_ck;

alter table intelligence_detection_records
  drop column probe_payload,
  drop column bank_id,
  drop column claimed_model,
  drop column margin,
  drop column detector,
  drop column verdict,
  alter column matched_phrases drop default;

alter table intelligence_detection_configs
  drop constraint intelligence_detection_configs_enforcement_ck,
  drop column enforcement;

-- Keep rollback compatible with legacy writers while allowing existing NULL
-- history to remain untouched, matching the original 0011 migration policy.
alter table intelligence_detection_records
  add constraint intelligence_detection_records_degraded_phrases_ck
  check (
    not degraded
    or (matched_phrases is not null and cardinality(matched_phrases) > 0)
  ) not valid;

commit;
