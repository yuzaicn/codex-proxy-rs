-- Replace the legacy phrase constraint before the fingerprint writer drops its
-- legacy-only fields. Existing rows receive legacy defaults and satisfy the
-- replacement cross-field constraints.
alter table intelligence_detection_records
  drop constraint intelligence_detection_records_degraded_phrases_ck;

alter table intelligence_detection_records
  alter column matched_phrases set default '{}'::text[],
  add column verdict text not null default 'legacy',
  add column detector text not null default 'legacy',
  add column margin double precision,
  add column claimed_model text,
  add column bank_id text,
  add column probe_payload jsonb;

alter table intelligence_detection_records
  add constraint intelligence_detection_records_verdict_ck
    check (verdict in ('normal', 'degraded', 'undetermined', 'unverifiable', 'legacy')),
  add constraint intelligence_detection_records_detector_ck
    check (detector in ('legacy', 'fingerprint')),
  add constraint intelligence_detection_records_legacy_pair_ck
    check ((detector = 'legacy') = (verdict = 'legacy')),
  add constraint intelligence_detection_records_fingerprint_degraded_ck
    check (detector <> 'fingerprint' or degraded = (verdict = 'degraded'));

alter table intelligence_detection_configs
  add column enforcement text not null default 'shadow',
  add constraint intelligence_detection_configs_enforcement_ck
    check (enforcement in ('shadow', 'enforce'));
