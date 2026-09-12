-- Preserve the reasoning stream used by the degradation decision for audit and review.
alter table intelligence_detection_records
  add column reasoning_content text;
