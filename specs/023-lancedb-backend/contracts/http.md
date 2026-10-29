# Storage HTTP v1

With a configured server token, all routes require `Authorization: Bearer <runtime credential>`. Without a server token, LAN clients connect without credentials. Credentials grant full access to this trusted shared index. PROJECTS filters retain their existing meaning and are not an authorization boundary. Use TLS termination or a trusted private transport; do not expose plaintext bearer credentials publicly.

`GET /v1/info`: protocol=1, schema=1, model and dimensions. Clients compare configured model/dimensions before becoming usable.

`POST /v1/store`: serde-tagged storage operation (`op`, snake_case), with JSON fields matching the existing Store operations. Requests carry protocol/model/dimension assertions on every operation. Successful responses contain `value`; errors contain `error`. HTTP auth, payload and overload errors retain appropriate status codes; storage execution failures return a structured error. SQL-like predicates are the same trusted internal storage API used by the local adapters, available through the operator's trusted LAN or configured bearer-auth boundary.

Operations cover rows/count, keyword/hybrid, put/replace_file/retag_file/tag_repository/delete, maintenance, metadata reads/writes and compare_metadata. compare_metadata atomically compares the entire previous JSON value under the writer lock, replacing it only if equal; false means conflict. The client can rerun a pure metadata update closure after a definitive conflict, with bounded attempts. Other writes are not automatically retried after network failures, since a response can be lost after commit.

The server bounds request bodies and active operations. Cloned local Store state shares one writer queue; reads are concurrent. No cross-request or cross-collection transaction is claimed. Local administrative writers must coordinate with the service or handle busy failures.
