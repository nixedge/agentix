# Remote grocery contract

## Core

GroceryList carries household/team ID, opaque list ID, list name and at most2000 distinct validated items. Item IDs are opaque strings, not local UUIDs. GroceryPlan freezes household/list identity and one mutation. Rename/completion/delete freeze the original item including category, note and purchase state for preflight comparison; add freezes requested text. Identifiers have at most256 visible ASCII bytes, item text1024 UTF-8 bytes, note8192 bytes and list name256 bytes.

Groceries.snapshot reads current authority. Groceries.execute may be invoked only after a durable dispatch claim and returns Confirmed, Rejected or Uncertain. Confirmed requires a successful RPC plus verified readback. Rejected means the mutation was not sent; Uncertain may include a committed remote change. The HTTP client disables both redirect following and Reqwest protocol retries. HTTPS origin is fixed to www.ourgroceries.com; the opt-in test-support constructor accepts only loopback HTTP and uses synthetic credentials.

Model context contains at most32 remote items/6144 estimated bytes and omits notes/categories. Original full fields stay in the execution snapshot for validation/preservation. Aliases such as groceries, grocery list, shopping list and their ordinary article/possessive prefixes route to the configured provider. Ambiguous equal-name/equal-state targets require owner disambiguation with id:ITEM_ID. Explicit IDs still must exist in the current snapshot.

## Journal transitions

Received → GroceryPrepared → GroceryDispatched → GroceryCompleted.

Preparation persists the immutable validated plan. Dispatch claim atomically transitions once and flushes before returning true. Further claims return false. Completion atomically stores the typed outcome, deterministic response, assistant message and released conversation marker, then flushes. Identical completed requests return the original response; mismatched envelopes or plans conflict. Local completion cannot finalize a grocery turn or manufacture local receipts.

On restart, a prepared plan can claim dispatch; an already dispatched plan finalizes uncertainty without another send. If completion acknowledgement is lost, begin replays the stored completed response. Failure before/after a flush can conservatively produce uncertainty even when nothing was sent. No exactly-once/CAS guarantee is offered by the reference API.

## Existing HTTP v1 / Android

POST /v1/assistant/tell request identity, auth behavior and shape remain unchanged. Grocery results use status=answer with source-explicit deterministic reply, outcomes=[] and records=[]. Local saves continue to require actual CommandReceipt values. A definitive uncertain grocery reply explicitly says to inspect the list before repeating; the Android queue validates identity, clears its pending envelope and can then issue a fresh read. No fabricated local grocery records or receipt IDs appear in the wire response.

## Remote HTTP

Sign in with form POST /sign-in using emailAddress/password/action=sign-in, capture only ourgroceries-auth, then GET /your-lists/ and parse the g_teamId JSON string. RPC POST /your-lists/ uses command/teamId/listId. Read discovery uses getOverview/shoppingLists; selected reads use getList/list/items. Mutations use insertItem, changeItemValue, setItemCrossedOff and deleteItem. Rename includes original categoryId; completion uses an explicit crossedOff boolean. Maximum body2MiB, connection timeout5s and request timeout10s. Application snapshot timeout25s and mutation timeout60s bound admitted work; durable recovery preserves the original identity if HTTP waiters leave.
