# Google Drive mirror setup

Set up the optional Google Drive mirror so every backup is also uploaded to your
own Drive. The whole setup fits in Google Cloud's free tier: the Drive API has no
usage charges and the project needs no billing account.

You end up with four values for `config.toml`: `client_id`, `client_secret`,
`refresh_token` and `folder_id`.

> **Keep the client secret and refresh token private.** Store them in a password
> manager. Never commit them, paste them in chats or include them in logs.

## Quick path

| Step | Where | Result |
|------|-------|--------|
| 1. Create a project and enable the Drive API | Google Cloud Console or `gcloud` | Project with Drive API enabled |
| 2. Configure the consent screen | Google Auth Platform | App created (External) |
| 3. Publish homepage and privacy pages | GitHub Pages (or any site you own) | Two public URLs |
| 4. Fill in Branding and publish the app | Google Auth Platform | App "In production" |
| 5. Create the OAuth client | Google Auth Platform → Clients | Client ID and secret |
| 6. Get the refresh token | OAuth 2.0 Playground | Refresh token |
| 7. Create the target folder (optional) | OAuth 2.0 Playground | Folder ID |
| 8. Configure the agent and verify | Server | `Google Drive upload completed` in the log |

## Cost and limits

| Item | What it means |
|------|---------------|
| Drive API usage | Free. Quotas are request rate limits; exceeding one fails the request temporarily and is never billed. |
| Upload limit | 750 GB per user per day. |
| Storage | Counts against your Google account storage (15 GB free, shared with Gmail and Photos). With `retention_days = 7` and weekly 2 GB backups, Drive holds 1–2 copies (2–4 GB). |
| Full storage | The upload fails and the job raises a Telegram alert. Nothing is charged; the local copy is kept. |

## 1. Create a project and enable the Drive API

Use a dedicated project so the agent's credentials stay isolated.

With `gcloud` (the project ID must be globally unique):

```sh
gcloud projects create backup-agent-drive-<suffix> --name="Backup Agent Drive"
gcloud services enable drive.googleapis.com --project=backup-agent-drive-<suffix>
```

Or in the console: create a project, then **APIs & Services → Library →
Google Drive API → Enable**.

Do not link a billing account; it is not needed.

> `gcloud` cannot create the OAuth consent screen or OAuth client for a personal
> Gmail account. Steps 2–5 are done in the console.

## 2. Configure the consent screen

1. Open `https://console.cloud.google.com/auth/overview?project=<PROJECT_ID>`.
2. Click **Get started** and complete the wizard:
   - **App information:** app name `Backup Agent`, your email as support email.
   - **Audience:** **External**.
   - **Contact information:** your email.
   - **Finish:** accept the Google API Services User Data Policy.
3. Click **Create**.

## 3. Publish homepage and privacy pages

Publishing the app (step 4) requires a valid **homepage URL** and **privacy
policy URL**, and their domain must be listed as an authorized domain.

This repository serves both pages from the `gh-pages` branch through GitHub Pages:

- Homepage: <https://cdcede.github.io/database-backup-agent/>
- Privacy policy: <https://cdcede.github.io/database-backup-agent/privacy.html>

Use them as they are, or host your own pages on a domain you control. Prefer a
`<user>.github.io` URL over `github.com`: you own the former and can authorize it.

## 4. Fill in Branding and publish the app

1. Go to **Branding** and fill in:
   - **App home page:** your homepage URL.
   - **App privacy policy link:** your privacy policy URL.
   - **Authorized domains:** the domain of those URLs (for example `cdcede.github.io`).
2. **Do not upload a logo.** A logo forces Google's app verification, which takes days.
3. Click **Save**.
4. Go to **Audience**, click **Publish app**, then **Confirm**.
5. The publishing status must read **In production**.

> **Why publish?** Refresh tokens of apps left in **Testing** expire after
> 7 days, and Drive uploads would start failing. The `drive.file` scope is
> non-sensitive, so publishing needs no verification.

## 5. Create the OAuth client

1. Open `https://console.cloud.google.com/auth/clients?project=<PROJECT_ID>`.
2. Click **Create client**.
3. **Application type:** **Web application**. **Name:** `Backup Agent`.
4. Under **Authorized redirect URIs**, add exactly:
   `https://developers.google.com/oauthplayground`
5. Click **Create**, then copy the **Client ID** and **Client secret**.
   Download the JSON if offered: the console may not show the full secret again.

## 6. Get the refresh token

1. Open <https://developers.google.com/oauthplayground>.
2. Click the **gear icon** (top right):
   - Check **Use your own OAuth credentials**.
   - Paste your **Client ID** and **Client secret**.
   - Leave **Access type** set to **Offline**.
3. In **Step 1**, type this into **Input your own scopes** and click
   **Authorize APIs**:

   ```text
   https://www.googleapis.com/auth/drive.file
   ```

   It looks like a URL, but it is the permission name: do not open it.
4. Sign in with your Google account and click **Allow**.
5. In **Step 2**, click **Exchange authorization code for tokens**.
6. Copy the **Refresh token** (it usually starts with `1//`). It also appears as
   `"refresh_token"` in the JSON response on the right.

> **Use the refresh token, not the access token.** The access token expires
> after one hour.
>
> No refresh token returned? **Access type** was not **Offline**. Fix it in the
> gear menu and repeat from step 6.3.

## 7. Create the target folder (optional)

Leave `folder_id` empty to upload to the Drive root.

To use a folder, **create it with your own OAuth client**. The `drive.file`
scope only grants access to files and folders the app created, so a folder
created by hand in the Drive web UI is invisible to the agent.

In the OAuth Playground, with your credentials still set:

1. Go to **Step 3 – Configure request to API**.
2. **HTTP Method:** `POST`. **Request URI:**
   `https://www.googleapis.com/drive/v3/files`
3. Click **Enter request body** and paste (rename the folder if you like):

   ```json
   {"name": "SQL Backups", "mimeType": "application/vnd.google-apps.folder"}
   ```

   Use `application/json` as the content type if asked.
4. Click **Send the request**.
5. Copy the `"id"` value from the response: that is your **Folder ID**.

The folder appears in your Drive. You can move it inside other folders; its ID
does not change.

> **Not recommended:** the broad `https://www.googleapis.com/auth/drive` scope
> lets the agent use any existing folder, but it grants access to your entire
> Drive and is a restricted scope (users see an "unverified app" warning). A
> leaked token would expose everything.

## 8. Configure the agent and verify

Back up the current config first:

```bat
copy "C:\Program Files\BackupAgent\config.toml" "%USERPROFILE%\config.toml.bak"
```

**Option A — GUI:** Settings → check **Also copy to Google Drive** → paste the
values → Save.

> Saving from the GUI rewrites `config.toml` entirely: comments are lost and the
> `[service]` section resets to defaults.

**Option B — `config.toml`:**

```toml
[storage.google_drive]
enabled = true
client_id = "YOUR_CLIENT_ID"
client_secret = "YOUR_CLIENT_SECRET"
refresh_token = "YOUR_REFRESH_TOKEN"
folder_id = "YOUR_FOLDER_ID"   # empty = Drive root
```

**Verify:** trigger a manual backup, then check:

- [ ] `backup-agent.log` (next to the service exe) contains `Google Drive upload completed`.
- [ ] The `.zip` file is in the target Drive folder.

## Retention on Drive

`retention_days` also applies to Drive. After a successful upload, that
database's expired `<db>_YYYYMMDD_HHMMSS.zip` files in the target folder are
**permanently deleted** (not trashed, so the quota is freed). Other files in the
folder are never touched, and a failed upload never deletes anything.

## Troubleshooting

| Symptom | Cause and fix |
|---------|---------------|
| Uploads fail after about 7 days | App still in **Testing**. Publish it (step 4) and get a new refresh token (step 6). |
| "Publish app" asks for missing branding info | Homepage URL, privacy policy URL or authorized domain missing (steps 3–4). |
| No refresh token in the Playground | **Access type** was not **Offline** (step 6.2). |
| Upload fails with a not-found error on the folder | The folder was created by hand. Create it through the app (step 7). |
| Access must be revoked | Remove the app at <https://myaccount.google.com/permissions>, then repeat step 6. |
