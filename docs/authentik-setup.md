# Authentik OIDC Setup for Anvil

This is the one-time manual setup performed in Authentik before turning on
`oidc.enabled` in the Anvil Helm chart. It covers the Provider, Application,
secrets, and Helm values. Once it's done, the panel will redirect every
unauthenticated visitor through Authentik for SSO.

Audience: panel operator (you). Replace every `<...>` placeholder with the
real value for your install.

| Variable          | Example                                          |
|-------------------|--------------------------------------------------|
| `<anvil-host>`    | `anvil.cherkaoui.ch`                             |
| `<authentik-host>`| `authentik.cherkaoui.ch`                         |
| `<namespace>`     | `anvil` (the namespace the chart is installed in)|

## 1. Create the OAuth2/OpenID Provider in Authentik

In **Applications → Providers → Create → OAuth2/OpenID Provider**:

| Field                       | Value                                                    |
|-----------------------------|----------------------------------------------------------|
| Name                        | `anvil`                                                  |
| Authorization flow          | `default-provider-authorization-explicit-consent` (or your preference) |
| Client type                 | Confidential                                             |
| Client ID                   | (auto-generated — note it)                               |
| Client Secret               | (auto-generated — note it; reveal once and copy)         |
| Redirect URIs / Origins     | `https://<anvil-host>/api/auth/callback`                 |
| Signing key                 | `authentik Self-signed Certificate` (or your CA)         |
| Subject mode                | Based on the User's UUID (default)                       |
| Include claims in id_token  | ✓ (default)                                              |
| Scopes (offered)            | `openid`, `email`, `profile`                             |

Save. From the provider detail page, also note the **Issuer URL** — it's
something like `https://<authentik-host>/application/o/anvil/`.

## 2. Create the Application

In **Applications → Applications → Create**:

| Field      | Value                              |
|------------|------------------------------------|
| Name       | `Anvil`                            |
| Slug       | `anvil`                            |
| Provider   | `anvil` (the provider above)       |
| Launch URL | `https://<anvil-host>`             |

Optionally bind a policy (group / user / expression) so only the people you
intend can see and use the app.

## 3. (Optional) Note your Authentik subject UUID

If you want to lock Anvil to specific users beyond the application binding,
you'll set `oidc.allowedSubs` to a comma-separated list of Authentik subject
UUIDs. Find your UUID in **Directory → Users → \<your-user\>** — the URL ends
in the UUID, and the detail page also shows it. Leaving `allowedSubs` empty
allows any user the application is bound to.

## 4. Generate the session key

```bash
openssl rand -base64 32
```

This is `ANVIL_SESSION_KEY`. Treat it like a password. Rotating it
invalidates every active session immediately, which is the right tool when
something looks compromised.

## 5. Stash the secrets

The chart accepts secrets two ways:

**Recommended (production)** — pre-create the Secret out-of-band and point
the chart at it via `oidc.existingSecret`:

```bash
kubectl -n <namespace> create secret generic anvil-oidc \
  --from-literal=ANVIL_OIDC_CLIENT_SECRET='<client-secret-from-step-1>' \
  --from-literal=ANVIL_SESSION_KEY='<openssl-rand-output-from-step-4>'
```

**Alternative (lab)** — pass values directly via `--set` or a values file
(`oidc.clientSecret`, `oidc.sessionKey`). The chart will create + manage a
Secret named `<release>-anvil-oidc`. This is fine for a homelab but means
the values file holds plaintext secrets; treat it accordingly.

## 6. Helm values

Minimal `values.yaml` overlay turning on OIDC and TLS:

```yaml
ingress:
  enabled: true
  host: <anvil-host>
  tls:
    enabled: true
    certResolver: letsencrypt   # Traefik certResolver, NOT cert-manager

mcDefaults:
  storageClassName: tank        # required regardless of OIDC

oidc:
  enabled: true
  issuerUrl: https://<authentik-host>/application/o/anvil/
  clientId: <client-id-from-step-1>
  redirectUrl: https://<anvil-host>/api/auth/callback
  allowedSubs: ""               # or your Authentik UUID(s), comma-separated
  existingSecret: anvil-oidc    # if you used step 5's Recommended path
```

Apply:

```bash
helm upgrade --install anvil deploy/ -n <namespace> -f values.yaml
```

If you skip `ingress.tls.enabled: true` while `oidc.enabled: true`, the
chart will refuse to render — by design, since the cookie security flags
are pointless without HTTPS.

## 7. Verify

- Visit `https://<anvil-host>/` in a private window. You should be
  redirected to Authentik.
- Log in. You should land back on `/`, with your name and avatar in the
  top-right corner.
- Click **sign out**. The browser should hit Authentik's end-session URL,
  end the IdP session, and (per Authentik's post-logout redirect) come
  back to the panel.

Quick sanity checks from a shell:

```bash
# Public — should be 200.
curl -sI https://<anvil-host>/api/health

# Protected without a cookie — should be 401.
curl -sI https://<anvil-host>/api/servers
```

## 8. Troubleshooting

| Symptom                                      | Likely cause / fix                                                                                            |
|----------------------------------------------|----------------------------------------------------------------------------------------------------------------|
| `redirect_uri mismatch` from Authentik       | Provider's **Redirect URIs** field must equal `oidc.redirectUrl` byte-for-byte (scheme, host, path, no trailing slash). |
| `invalid_client` from Authentik              | Client secret stale or mistyped; re-copy from the Authentik provider detail page.                              |
| 401 on every request after a successful login | `ANVIL_SESSION_KEY` rotated since the cookie was minted. Log in again.                                         |
| 403 with `code: sub_not_allowed`             | Your subject UUID is not in `oidc.allowedSubs`. Either add it, or set `allowedSubs: ""`.                        |
| 502 from `/api/auth/callback`                | Backend can't reach Authentik for token exchange. Check pod egress + DNS for `<authentik-host>`.               |
| Helm template fails with `oidc.enabled requires ingress.tls.enabled` | Turn TLS on, or turn OIDC off. The two are gated together by design. |
