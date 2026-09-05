# Connectors tab design

Approved intent: reusable SDK; personal daemon integrations; main Connectors navigation for Telegram, Google Calendar, Gmail and Outlook. Mail can read and draft; every send requires owner approval.

UI: existing responsive shell gains Connectors. Telegram management moves from Settings; Telegram conversation stays separate. OAuth service cards show setup availability, account/status, connect/reconnect/disconnect. Calendar exposes pending write approvals. Mail displays recent inbox messages and durable local drafts, with explicit Send/Reject review. Drafts stay in animaOS until approved; do not mutate provider drafts. No automatic email sending. Connections are scoped to the current main agent, matching existing daemon ownership. Status refresh remains active while visible; daemon refresh/polling continues independently.

Daemon: Gmail and Outlook implement OAuth authorization code with state expiry and PKCE, OS vault token storage, refresh token rotation, bounded inbox reads, restart recovery and explicit reauth status. Shared mail manager uses per-connector locking and persisted state. Send claims must be durably recorded before provider call; ambiguous sends must not automatically retry. Disconnect invalidates pending authorization and work. No network side effects until owner guard and lifecycle checks. Provider errors must be sanitized. Existing Calendar remains supported.

API contract (prefix /api/agents/{agentId}/connectors):
- GET /mail/{provider} -> {configured:boolean,connector:MailConnector|null}; provider is gmail|outlook.
- POST /mail/{provider} -> {connector,consentUrl}; start/restart OAuth. Callback GET /api/connectors/mail/{provider}/callback.
- DELETE /mail/{provider}/{connectorId} -> {deleted:true}.
- GET /mail/{provider}/{connectorId}/messages -> {messages:MailMessage[]} (bounded recent inbox, explicit refresh supported by daemon).
- GET /mail/{provider}/{connectorId}/drafts -> {drafts:MailDraft[]}.
- POST /mail/{provider}/{connectorId}/drafts with {to:string[],subject:string,body:string} -> {draft} (local durable draft).
- POST /mail/{provider}/{connectorId}/drafts/{draftId}/approve -> {draft}; owner-only sends immutable draft once.
- POST /mail/{provider}/{connectorId}/drafts/{draftId}/reject -> {draft}.
MailConnector: {id,agentId,type:gmail|outlook,accountLabel:string|null,status:pairing|active|reauthRequired,createdAtMs,updatedAtMs,lastSyncedAtMs?:number|null,error?:string|null}.
MailMessage: {id,from,subject,preview,receivedAt:string}; text only.
MailDraft: {id,connectorId,to:string[],subject,body,state:pending|sending|sent|rejected|failed|unknown,error:string|null,createdAtMs,resolvedAtMs:number|null}.
All mail operations require approved local owner authorization plus normal API auth. Error envelope {error:string}, proper 400/403/404/409/503. No secrets in responses or snapshots. Do not expose approvals as agent-callable tools.

SDK: add ConnectorsClient with existing Calendar operations and new Mail operations/types. Web consumes these SDK methods using same-origin base URL (no hardcoded daemon port). Telegram remains wired to proven existing client during this change.

OAuth setup: Google ANIMA_GOOGLE_CLIENT_ID/SECRET plus mail callback URI separate from Calendar; Outlook ANIMA_MICROSOFT_CLIENT_ID/SECRET with optional tenant common. Expose unavailable state until configured. Live consent must be completed by user; do not request credentials in chat or send mail during testing.

Validation: transport fake tests for OAuth replay/expiry, persistence, disconnect, refresh, send approval/replay/ambiguity; UI integration tests for four cards/status/mutations/stale responses; SDK request/type tests; Nx web, SDK and Rust validations, isolated runtime smoke check and restart actual dev stack.
