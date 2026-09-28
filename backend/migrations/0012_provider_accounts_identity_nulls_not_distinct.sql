-- Make provider-account imports idempotent even when token identity claims are absent.
-- Keep the most recently created row before enforcing the stricter uniqueness rule.
lock table provider_accounts in share row exclusive mode;

create temp table provider_account_identity_duplicates on commit drop as
select duplicate.id as duplicate_id, survivor.id as survivor_id
from provider_accounts duplicate
join lateral (
  select candidate.id
  from provider_accounts candidate
  where candidate.provider_kind = duplicate.provider_kind
    and candidate.upstream_user_id is not distinct from duplicate.upstream_user_id
    and coalesce(candidate.upstream_account_id, '') = coalesce(duplicate.upstream_account_id, '')
  order by candidate.created_at desc, candidate.id desc
  limit 1
) survivor on survivor.id <> duplicate.id;

-- Account-group membership has a composite primary key, so merge memberships before
-- deleting duplicate accounts and collapse rows that already point at the survivor.
insert into account_group_accounts (account_group_id, provider_account_id, created_at)
select membership.account_group_id, duplicate.survivor_id, min(membership.created_at)
from account_group_accounts membership
join provider_account_identity_duplicates duplicate
  on duplicate.duplicate_id = membership.provider_account_id
group by membership.account_group_id, duplicate.survivor_id
on conflict (account_group_id, provider_account_id) do update
set created_at = least(account_group_accounts.created_at, excluded.created_at);

delete from account_group_accounts membership
using provider_account_identity_duplicates duplicate
where membership.provider_account_id = duplicate.duplicate_id;

update model_requests request
set provider_account_id = duplicate.survivor_id,
    provider_account_ref = duplicate.survivor_id
from provider_account_identity_duplicates duplicate
where request.provider_account_id = duplicate.duplicate_id;

update ops_events event
set provider_account_id = duplicate.survivor_id,
    provider_account_ref = duplicate.survivor_id
from provider_account_identity_duplicates duplicate
where event.provider_account_id = duplicate.duplicate_id;

update intelligence_detection_records record
set account_id = duplicate.survivor_id
from provider_account_identity_duplicates duplicate
where record.account_id = duplicate.duplicate_id;

delete from provider_accounts account
using provider_account_identity_duplicates duplicate
where account.id = duplicate.duplicate_id;

do $$
begin
  if exists (
    select 1
    from provider_accounts
    group by provider_kind, upstream_user_id, coalesce(upstream_account_id, '')
    having count(*) > 1
  ) then
    raise exception 'provider account identity deduplication left duplicate rows';
  end if;
end
$$;

drop index provider_accounts_upstream_identity_uq;
create unique index provider_accounts_upstream_identity_uq
  on provider_accounts (
    provider_kind,
    upstream_user_id,
    coalesce(upstream_account_id, '')
  ) nulls not distinct;
