-- Configure the reasoning effort used by intelligence detection probes.
alter table intelligence_detection_configs
  add column reasoning_effort text not null default 'auto';
