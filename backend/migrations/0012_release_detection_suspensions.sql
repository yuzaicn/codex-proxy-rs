-- 检测 Worker 下线后，释放仅由该功能施加的调度暂停。
-- 保留手动暂停及检测历史；同账号仍通过 enabled/status 控制基础调度资格。
update provider_accounts
set scheduling_suspended = false,
    scheduling_suspended_by = null
where scheduling_suspended = true
  and scheduling_suspended_by = 'detection';
