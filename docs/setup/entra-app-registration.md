# Registering the Entra app

ms-todo signs in to Microsoft Graph through an app registration in Microsoft Entra ID. The registration gives it an **Application (client) ID**. You don't need a client secret: ms-todo is a public client using device-code sign-in.

## Current maintainer registration

BK registered `ms-todo` in his personal Default Directory on 2026-09-24. Its **Application (client) ID is `48d9179b-67f3-4969-985e-9690aff42435`**. This ID is public and is recorded here so a build on another machine can use it. The app supports organizational and personal Microsoft accounts, allows public-client flows, and has delegated `Tasks.ReadWrite`, `MailboxSettings.ReadWrite`, `offline_access` and `User.Read` permissions. It has no redirect URI or client secret.

The ID is also in the `ms-todo Entra app` item in 1Password's `Environment Variables` vault, under `client_id`, and in a local `.env` on the registration machine as `MS_TODO_CLIENT_ID`. The `.env` file is ignored by Git and does not travel with a clone. Device-code sign-in and Graph reads succeeded on 2026-09-24; the sanitized results are in [spike S5](../blueprint/12-open-questions.md#s5-result-2026-09-24). The rest of the Phase 0 spikes ran with the same registration. The token response's `scope` leaves out `offline_access`, but a refresh token is issued anyway.

It takes about 10 minutes. You do it once. Release builds of ms-todo include the maintainer's client ID, but we recommend registering your own (see "Why your own" at the end).

Sources: [Register an app](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app) (updated 2026-06-15), [Device code flow](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code), [Graph permissions reference](https://learn.microsoft.com/en-us/graph/permissions-reference).

## Before you start: which tenant?

**Don't register this app in your employer's tenant.** If you're signed in to entra.microsoft.com with a work account, the app would belong to your employer's directory, and its admins would control it. It could also break company policy. Register it in a directory you personally own.

- **You have a personal Microsoft account (outlook.com, hotmail, live).** Sign in to https://entra.microsoft.com with it. If you've used Azure before, you already have a free "Default Directory", and you can go straight to step 1.
- **You have no directory yet.** Microsoft's guide lists "An Azure account that has an active subscription" as a prerequisite. The free Azure signup at https://azure.microsoft.com/free creates the directory. It asks for a card to verify your identity (a temporary authorisation, reversed afterwards); app registrations themselves are free. This is the one uncertain step. See the registration section of `docs/research/microsoft-todo-api.md`.
- To check which directory you're in, look at the account menu in the top right of the admin centre. The **Settings** (gear) icon switches between directories.

## Step 1: Create the registration

1. Go to https://entra.microsoft.com.
2. Go to **Entra ID** → **App registrations**, and choose **New registration**.
3. **Name:** `ms-todo`. People see this name on the consent screen, and you can change it later.
4. **Supported account types:** choose **Any Entra ID Tenant + Personal Microsoft accounts**.
   - This is what lets ms-todo use the `/common` endpoint and work with personal and work accounts alike. Pick anything else and a personal account fails at sign-in with an error saying the app isn't configured for Microsoft accounts.
   - Want the tightest scope instead? Choose **Personal accounts only**. You'd then have to set `auth.authority = "consumers"` in ms-todo's config. The blueprint assumes `/common`, so stick with the first option unless you have a reason.
5. **Redirect URI:** leave it empty. Device-code sign-in doesn't use one.
6. Choose **Register**.
7. On the **Overview** page, copy the **Application (client) ID**, a GUID. You'll need it in step 4. The Directory (tenant) ID isn't needed.

## Step 2: Allow public client flows (required for device code)

1. In the app's menu, open **Authentication**.
2. Find **Advanced settings**, then **Allow public client flows**. In the newer Authentication page this is under the **Settings** tab.
3. Set **Enable the following mobile and desktop flows** to **Yes**.
4. Choose **Save**.

If you skip this, `ms-todo auth login` fails when it asks for a token, with an error like **AADSTS7000218**: *"The request body must contain the following parameter: 'client_assertion' or 'client_secret'"*. Entra is treating the app as a confidential client that needs a secret.

**Don't** create a client secret (Certificates & secrets). ms-todo doesn't use one, and a secret would only be something that could leak or expire.

## Step 3: Add API permissions

1. Open **API permissions**. The app already has **Microsoft Graph → User.Read**. Keep it.
2. Choose **Add a permission** → **Microsoft Graph** → **Delegated permissions**.
3. Search for and tick each of these:

   | Permission | Why |
   |---|---|
   | `Tasks.ReadWrite` | Read and write your lists and tasks |
   | `MailboxSettings.ReadWrite` | Create and manage Outlook categories: `@labels`, and the "My Day" category |
   | `offline_access` | Get refresh tokens, so you stay signed in (it's under **OpenId permissions**) |
   | `User.Read` | Already there. Shows who is signed in |

4. Choose **Add permissions**.
5. **You don't need admin consent** for a personal account. None of these need it, and you consent yourself the first time you sign in. Ignore the "Grant admin consent" button unless you're on a work tenant you administer.

**Don't add** `Tasks.ReadWrite.Shared` (sharing was dropped, D-014), any application permissions (To Do doesn't support app-only writes), or `Tasks.ReadWrite.All`.

The permissions you add here are only the list of what the app may request. ms-todo requests the scopes itself at sign-in (`offline_access Tasks.ReadWrite MailboxSettings.ReadWrite User.Read`), and that's what shows on the consent screen.

## Step 4: Save the client ID

The client ID isn't a secret. For this project's maintainer registration, use the ID recorded above. Optionally keep it in your own credential store:

```sh
# Replace the vault name with one available in your 1Password account.
# BK: 1Password writes need the desktop profile and Touch ID.
op item create --category "API Credential" --title "ms-todo Entra app" \
  --vault "Environment Variables" "client_id[text]=<GUID>"
```

ms-todo takes it from any of these, highest priority first:

1. the `MS_TODO_CLIENT_ID` environment variable, e.g. `export MS_TODO_CLIENT_ID=$(op read "op://Environment Variables/ms-todo Entra app/client_id")`
2. `auth.client_id` in `<config_dir>/ms-todo/config.toml`
3. the ID built into release builds

## Step 5: Check it works (before any ms-todo code exists)

Try the device code flow with `curl`. It's also the start of spike S5:

```sh
umask 077
CID=48d9179b-67f3-4969-985e-9690aff42435
curl -fsS https://login.microsoftonline.com/common/oauth2/v2.0/devicecode \
  --data-urlencode "client_id=$CID" \
  --data-urlencode 'scope=offline_access Tasks.ReadWrite MailboxSettings.ReadWrite User.Read' \
  -o /tmp/dc.json
jq '{user_code, verification_uri, expires_in, interval}' /tmp/dc.json
# Open https://microsoft.com/devicelogin, enter the user_code, sign in, and consent.
# Personal accounts are asked to sign in twice. That's expected (Microsoft's docs note it).
curl -sS https://login.microsoftonline.com/common/oauth2/v2.0/token \
  --data-urlencode 'grant_type=urn:ietf:params:oauth:grant-type:device_code' \
  --data-urlencode "client_id=$CID" \
  --data-urlencode "device_code=$(jq -r .device_code /tmp/dc.json)" \
  -o /tmp/tok.json
jq '{token_type, expires_in, scope, error, error_description}' /tmp/tok.json
curl -fsS -H "Authorization: Bearer $(jq -r .access_token /tmp/tok.json)" \
  https://graph.microsoft.com/v1.0/me/todo/lists | jq '{list_count:(.value|length)}'
curl -fsS -H "Authorization: Bearer $(jq -r .access_token /tmp/tok.json)" \
  https://graph.microsoft.com/v1.0/me/outlook/masterCategories | jq '{category_count:(.value|length)}'
rm /tmp/dc.json /tmp/tok.json   # these contain live tokens
```

If you run the second `curl` before finishing sign-in, you get `authorization_pending`. That's normal: run it again after you've signed in. Successful list and category counts mean the registration and the permissions used by S5 work.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `AADSTS7000218` … `client_secret` | Public client flows are off | Step 2 |
| "The application … is not configured as a multi-tenant application" or "not supported for Microsoft accounts" | Wrong **Supported account types** | App → **Authentication** (or the **Manifest**: `signInAudience` must be `AzureADandPersonalMicrosoftAccount`) |
| `AADSTS65001` consent / `invalid_grant` on first use | Consent wasn't completed | Run sign-in again and accept the consent screen |
| Consent screen says **unverified** | Multi-tenant apps without a verified publisher show this | Expected for your own app. It matters for the bundled ID (below) |
| A work account is blocked from consenting | That tenant's admin doesn't allow user consent to unverified apps | Use a personal account, or ask the admin |
| `ErrorAccessDenied` from Graph on `/me/todo` | `Tasks.ReadWrite` wasn't in the requested scopes | Check the `scope` sent at sign-in |

## Why your own registration (and what the bundled ID means)

Release builds include the maintainer's client ID, so `brew install` followed by `ms-todo auth login` works straight away (D-025). But with the bundled ID:

- you consent to *the maintainer's* app registration, and it keeps working only as long as the maintainer keeps it;
- the consent screen shows it as unverified (there's no verified publisher);
- work tenants that block consent to unverified apps won't let you sign in.

Your own registration avoids all three and takes 10 minutes. `ms-todo auth status` shows which client ID is in use.
