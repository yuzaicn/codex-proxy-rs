-- 账号调度暂停列（与人工 enabled 正交，由手动调度开关或降智检测 Worker 翻转）
alter table provider_accounts
  add column scheduling_suspended boolean not null default false,
  add column scheduling_suspended_by text;  -- 'manual' | 'detection'，未暂停时为 null

-- 降智检测全局配置（单行）
create table intelligence_detection_configs (
  id              bigserial primary key,
  enabled         boolean not null default false,
  -- account_scope: {"all": true} 或 {"account_ids": ["id1", "id2"]}
  account_scope   jsonb not null default '{"all": true}',
  interval_secs   integer not null default 3600,
  model           text not null default '',
  updated_at      timestamptz not null default now()
);

-- 检测记录
create table intelligence_detection_records (
  id                  bigserial primary key,
  detection_round_id  uuid not null,
  account_id          text not null references provider_accounts(id),
  checked_at          timestamptz not null default now(),
  degraded            boolean not null,
  html_content        text,
  matched_phrases     text[]
);
create index on intelligence_detection_records (detection_round_id, checked_at desc);
create index on intelligence_detection_records (account_id, checked_at desc);
