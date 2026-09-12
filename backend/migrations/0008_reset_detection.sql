-- 重置卡检测调度配置：单行单例，模式与 backup_settings 一致（0001_initial.sql:784 起）。
-- 归属：配置事实，写路径经 admin use_case 推进 config_revision。
create table reset_detection_settings (
  id bigint primary key,
  enabled boolean not null default false,
  poll_interval_secs bigint not null default 3600,
  account_scope text not null default 'all_non_error',
  auto_consume_enabled boolean not null default false,
  updated_at timestamptz not null,
  constraint reset_detection_settings_singleton_ck check (id = 1),
  -- 下限 30 秒：库层只挡灾难性误配置；上游调用预算由 Worker 侧并发与抖动控制（GUCH-129）。
  constraint reset_detection_settings_interval_ck check (poll_interval_secs >= 30),
  constraint reset_detection_settings_scope_ck check (
    account_scope in ('all_non_error', 'normal', 'limited')
  )
);

insert into reset_detection_settings (id, updated_at) values (1, now());

-- 账号行重置卡观测：运行时观测值（与 provider_quota_json / quota_observed_at 同类），
-- 不是卡库存台账，权威仍是 OpenAI upstream；未观测过 = 两列同为 null（区别于 0）。
alter table provider_accounts
  add column reset_credits_available_count bigint,
  add column reset_credits_observed_at timestamptz;

alter table provider_accounts
  add constraint provider_accounts_reset_credits_observation_ck check (
    ((reset_credits_available_count is null) = (reset_credits_observed_at is null))
    and (reset_credits_available_count is null or reset_credits_available_count >= 0)
  );
