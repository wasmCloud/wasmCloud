# NATS KV bucket contracts

The [`kv` and `kv-admin` WIT comments](wit/world.wit) define the component-facing API.
This document defines the operator policy used by the `wasmcloud:nats` host plugin.
Creating buckets requires NATS 2.10 or newer; per-key TTL requires 2.11 or newer.

## Permissions

Each operation must satisfy every grant in its row.
All three grants deny by default.

| Operation | Required grants |
| --- | --- |
| Open a bucket; read, write, delete or purge keys; watch; inspect bucket configuration | `bucket-allow` |
| Create or ensure a bucket | `bucket-allow` **and** `bucket-create-allow` |
| Delete a bucket and all its keys | `bucket-allow` **and** `bucket-delete-allow` |

`bucket-allow` grants use of a bucket, including writes and key deletion.
It is not a read-only grant.
Creation and whole-bucket deletion are independent permissions; neither implies the other.
The host checks the operation's grants before looking up whether the bucket exists.
NATS credentials must also permit the operation.

`bucket-create-allow` accepts exact names or `*`, which grants creation of any valid bucket name.
Adding exact names alongside `*` does not restrict it.
Other patterns such as `tenant-*` and `>` are refused for creation.
`bucket-allow` and `bucket-delete-allow` accept NATS subject patterns; `*` covers all valid bucket names.

## Operator policy and workload narrowing

Use `workloadConfig: deny` (the default) to enforce operator-owned configuration and ceilings.
A workload may narrow the bucket grants, size ceiling and allowed storage types.
It may not set the replica count or widen a ceiling.
`warn` and `allow` permit workload overrides; they do not enforce these ceilings.

Permissions and resource obligations are separate:
**narrowing permitted bucket names never removes an account-budget requirement established by the operator.**
The host derives that requirement from the operator's declaration; there is no extra configuration switch.
Secret-supplied creation grants conservatively retain the same requirement because startup validation cannot inspect their value.
A binding with no effective creation grant does not need a creation budget.

| Operator creation grant | Workload creation grant | Result under `deny` | Account budget required |
| --- | --- | --- | --- |
| `*` | omitted | Inherit `*` | Yes |
| `*` | `cache` | Only `cache` | Yes |
| `*` | empty | No creation | No |
| `cache` | omitted or `cache` | Only `cache` | No wildcard preflight |
| `cache` | `*` | Reject deployment | — |

Named bindings use their own operator layer over the base.
An unnamed binding, or an undeclared label where labels are permitted, uses the base's requirements.
The requirement survives partial validation of other bindings.

## Resource limits and account isolation

Every binding with a creation grant must explicitly set `bucket-max-bytes` to a positive byte count or `unlimited`.
`bucket-storage-allow` accepts `file`, `memory`, or both, and defaults to `file`.
`bucket-replicas` is operator-only, accepts 1–5, and defaults to one.
These settings apply to each new bucket, including wildcard creation.

Exact creation names bound the set of names a workload can create.
Wildcard creation instead requires positive account limits for stream count and each allowed storage type.
The host checks the selected replica tier's limits when tiers are configured.
If the account information cannot be read or the required limits are unbounded, binding fails.
This is a preflight check of configured limits, not a reservation of free capacity.
NATS checks its current limits on every create; the host does not reserve capacity or maintain a local bucket count.
Account or server resource refusals are reported as `limit-exceeded`, including per-stream byte limits, required byte limits and unavailable replica-tier limits.

Quota admission has the concurrency guarantees of the deployed NATS server.
On single-node [NATS 2.12.15](https://github.com/nats-io/nats-server/blob/v2.12.15/server/jetstream_api.go#L1693-L1699), two concurrent creates of different bucket names can both pass the last available stream-slot check.
The configured stream count must therefore not be treated as an atomic admission cap under concurrent creation on that server.
A strict cross-host admission guarantee requires broker support or a separate coordinator; this plugin provides neither.

A KV bucket consumes a stream, and all streams in the account share its budget.
Separate bindings, workloads, hosts, or credentials in the same NATS account do not get independent budgets.
Give independently budgeted tenants separate NATS accounts.
`bucket-allow: "*"` also permits use of existing buckets in that account.

## Configuration precedence

For a binding, values take precedence in this order, from lowest to highest:

1. Schema defaults, including file-only bucket storage.
2. Applicable operator default bundles.
3. Operator base configuration, then the named binding's configuration.
4. Workload configuration accepted by the configured workload policy.

Bundles fill missing keys only.
An explicit host or workload anchor, such as `servers`, suppresses its bundle, including associated TLS defaults.
Bundle exclusion keys also suppress the bundle.
Pending or withheld secrets count as set, so defaults cannot replace them.
The effective host layer, including applicable defaults, is the ceiling against which workload narrowing is checked.

An omitted workload value inherits the effective host value.
An explicitly empty bucket grant narrows it to no permission.
Omitted or blank host storage uses `file`; an explicitly empty workload storage list is rejected under `deny` rather than falling back to `file`.
`bucket-max-bytes` has no implicit creation allowance: zero and blank are invalid configuration values.
The guest API's `max-bytes: 0` is a separate instruction to use the binding's effective size limit.

## Create and ensure

`create-bucket` ensures a compatible bucket exists and opens it.
It never migrates a bucket, updates its configuration, or deletes data to reconcile a difference.
Grant checks occur first, followed by request validation and storage policy checks.

| State or condition | Observable result |
| --- | --- |
| Any required grant missing, whether the bucket exists or not | `denied` |
| Requested storage outside the allowed types | `limit-exceeded`, whether the bucket exists or not |
| Absent bucket and valid permitted request | Create with the effective size limit and operator replica count; return a bucket handle |
| Existing compatible bucket | Return a handle; leave configuration and data unchanged |
| Existing incompatible bucket, including one above a lowered size ceiling | `config-conflict` carrying its actual configuration; leave it unchanged |
| Requested size above the binding ceiling, with no existing bucket | `limit-exceeded` |
| New bucket refused by an account or server resource limit | `limit-exceeded` |

For a new bucket, guest `max-bytes: 0` takes the effective binding ceiling, or unlimited when the binding explicitly permits it.
For an existing bucket, zero matches any size within that ceiling.
An unlimited existing bucket does not fit under a finite ceiling.
Replicas apply only to new buckets and never decide compatibility with an existing bucket.
Ensuring an existing compatible bucket needs no additional stream slot, including after a workload restart at a full stream quota.
History zero means one; marker TTL is rounded up to whole seconds before compatibility is compared.

## Contract coverage

These named tests protect the contracts above:

| Contract | Tests |
| --- | --- |
| Independent operation grants and default denial | `bucket_lifecycle_requires_bucket_allow_and_its_own_grant`; `kv_admin_creates_reads_and_deletes_buckets_under_their_grants` |
| Narrowing preserves budget requirements | `bucket_creation_narrowing_preserves_account_budget_requirements`; `kv_admin_a_narrowed_operator_wildcard_still_needs_a_bounded_account` |
| Binding defaults and narrowing use the same effective ceiling | `explicit_file_storage_fits_the_hosts_default_under_deny`; `blank_host_storage_keeps_the_file_default_narrowable`; `a_storage_bundle_sets_the_effective_host_ceiling`; `a_blank_storage_list_cannot_widen_the_operators` |
| Ensure preserves compatible buckets and reports conflicts | `kv_admin_creates_reads_and_deletes_buckets_under_their_grants`; `kv_admin_any_name_creation_is_bounded_by_the_account` |
| A full account refuses concurrent creates from both hosts and counts non-KV streams | `kv_admin_two_hosts_share_an_account_budget` |
| Account refusal categories map to `limit-exceeded` | `create_errors_classify_by_cause`; `kv_admin_a_bucket_over_the_accounts_per_stream_limit_is_limit_exceeded` |

Live NATS tests are marked ignored and run in the ignored-test CI job.
