# OAuth App Configuration Design

## Goal

Let the workspace owner configure Google and Microsoft OAuth applications entirely from the Connectors screen. The owner should never need to edit source code, set shell variables, or restart the daemon.

## Product behavior

The Connectors screen adds a compact **OAuth app setup** area above the service cards. Google is configured once and is shared by Gmail and Google Calendar. Microsoft is configured once for Outlook. Each setup card shows its exact redirect URI, client ID and client-secret inputs, and an optional Microsoft tenant field. After Save succeeds, the secret fields clear, the card reports **Configured**, and the affected service controls refresh immediately so Connect becomes available.

Saved secrets are write-only from the browser. A later GET reports only whether configuration exists, whether it came from the operating-system vault or the process environment, a short client-ID hint, the redirect URI, and the Microsoft tenant. It never returns a client secret. A vault-managed configuration can be replaced or removed from the UI. Environment-managed configuration can be viewed but not removed through the UI.

## Architecture

The Rust daemon owns a new OAuth application configuration service. Production uses Windows Credential Manager through the existing `keyring` dependency; tests use an in-memory implementation. Environment variables remain a backward-compatible fallback. Vault values take precedence so a UI save takes effect immediately.

The service exposes owner-only HTTP JSON endpoints under `/api/connectors/oauth-apps/{provider}`. The SDK wraps these endpoints. Calendar and mail managers resolve the current OAuth configuration from this shared service at operation time instead of retaining an immutable startup snapshot. This makes Save effective without restarting the daemon.

Google and Microsoft are daemon-level application settings, while connected accounts remain agent-scoped. Removing application settings is rejected while the corresponding provider has an active or pairing connector, preventing a silently broken connection. The owner disconnects accounts first, then removes the app credentials.

## API contract

`GET /api/connectors/oauth-apps/{provider}` where provider is `google` or `microsoft` returns:

```json
{
  "provider": "google",
  "configured": true,
  "source": "vault",
  "clientIdHint": "...abcd",
  "redirectUris": {
    "mail": "http://127.0.0.1:8080/api/connectors/mail/gmail/callback",
    "calendar": "http://127.0.0.1:8080/api/connectors/gcalendar/callback"
  },
  "tenant": null
}
```

`PUT /api/connectors/oauth-apps/{provider}` accepts `clientId`, `clientSecret`, and optional `tenant` for Microsoft. Both credential fields are required and trimmed. Values have conservative size limits, reject control characters, and Microsoft tenant accepts `common`, `organizations`, `consumers`, or a tenant ID/domain using the daemon's current safe character set. The operation replaces the provider's vault configuration atomically and returns the redacted status envelope.

`DELETE /api/connectors/oauth-apps/{provider}` removes only a vault-managed configuration and returns `204`. It returns `409` if a dependent connector exists or the effective configuration comes only from environment variables.

All three operations use the existing local-owner authorization boundary. They are included in OpenAPI. Responses use `400` for invalid input/provider, `403` for owner failure, `409` for a lifecycle conflict, and `503` for credential-vault failure. Secrets are zeroized in memory, excluded from debug output, omitted from logs and never included in response/error text.

## UI and SDK

The SDK adds typed `oauthAppStatus`, `configureOauthApp`, and `removeOauthApp` methods. The web view loads Google and Microsoft setup status independently so one provider failure does not hide the other. Forms use password inputs, browser autocomplete suitable for new secrets, disabled/busy states, and generic failure text. Redirect URIs remain visible before setup so the owner can paste them into Google Cloud or Microsoft Entra.

The Google card explains that one credential pair enables both Gmail and Calendar. The Microsoft card explains that it enables Outlook. Save never initiates OAuth automatically; after configuration, the owner deliberately clicks Connect on the desired service.

## Validation

Tests cover vault precedence over environment fallback, redaction, validation, replacement, removal conflicts, live manager reconfiguration, local-owner enforcement, SDK path/body behavior, and UI form behavior. Full Rust, SDK and web test/build targets run through Nx. A local daemon smoke test saves fake credentials, verifies only redacted status is returned, observes Gmail/Calendar readiness without restart, then removes the fake configuration without contacting an external provider.
