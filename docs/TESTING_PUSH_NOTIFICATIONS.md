# Testing Push Notifications with Postman

> A hands-on, copy-paste guide for verifying the full FCM pipeline end-to-end: register a device → trigger a task event → confirm a notification is delivered.

---

## Table of Contents

1. [Prerequisites](#1-prerequisites)
2. [How the push flow actually works](#2-how-the-push-flow-actually-works)
3. [Import the Postman collection](#3-import-the-postman-collection)
4. [Test matrix at a glance](#4-test-matrix-at-a-glance)
5. [Test 1 — Smoke test with a FAKE token (no real device needed)](#test-1--smoke-test-with-a-fake-token)
6. [Test 2 — Real device, foreground notification](#test-2--real-device-foreground-notification)
7. [Test 3 — Real device, background notification](#test-3--real-device-background-notification)
8. [Test 4 — Real device, terminated state](#test-4--real-device-terminated-state)
9. [Test 5 — Update triggers (status → completed, priority → high)](#test-5--update-triggers)
10. [Test 6 — Delete triggers a push](#test-6--delete-triggers-a-push)
11. [Test 7 — Idempotent device registration](#test-7--idempotent-device-registration)
12. [Test 8 — Unregister a device](#test-8--unregister-a-device)
13. [Common errors and what they mean](#13-common-errors-and-what-they-mean)
14. [Quick reference — all request bodies in one place](#14-quick-reference--all-request-bodies-in-one-place)

---

## 1. Prerequisites

Before testing pushes, confirm each item:

- ✅ **Postgres is running** on `localhost:5432` with database `task_db`.
- ✅ **Migrations applied** — `devices` table exists (you confirmed this in pgAdmin earlier).
- ✅ **`service-account.json`** sits in the project root and its `project_id` matches `FIREBASE_PROJECT_ID` in `.env` (currently `flutter-app-fcm-cfd6d`).
- ✅ **Server running**:
  ```powershell
  cargo run
  ```
  You should see `listening on 0.0.0.0:3000` in the terminal.
- ✅ **Postman installed**, with the [`task_api.postman.yaml`](../task_api.postman.yaml) imported (see step 3 below).

> **You do NOT need a real Flutter app** to test that the backend is wired correctly. Tests 1, 5, 6, 7, 8 all run with a fake token — failures from FCM with `404 UNREGISTERED` confirm the integration is working (the backend talked to Google, Google rejected the bogus token).

---

## 2. How the push flow actually works

```
┌────────────┐  POST /tasks    ┌────────────────────┐
│  Postman   │ ───────────────▶│   task_api (Rust)  │
└────────────┘                 │  • insert task     │
                               │  • return 201      │ ◀── client sees this immediately
                               │  • tokio::spawn ──┐│
                               └──────────────────┼┘
                                                  │ background task:
                                                  ▼
                                    SELECT token FROM devices
                                                  │
                                                  ▼
                                    POST fcm.googleapis.com/v1/projects/...
                                                  │
                                                  ▼
                                    For each device: log success/error
```

**Two important consequences:**

1. **The HTTP response is independent of FCM.** Your Postman call returns 201 even if every push fails. To see what FCM did, you watch the **server logs** in the `cargo run` terminal.
2. **You need at least one row in `devices`** before any push fires. Empty table → broadcast loop runs zero times → no FCM call.

---

## 3. Import the Postman collection

1. Open Postman.
2. **File → Import** → drop `c:\Rust Projects\project\task_api\task_api.postman.yaml` in.
3. Pick **OpenAPI 3.0/3.1** → **Generate collection** → **Import**.
4. You'll get a collection named `task_api` with seven requests. Each has named example bodies in the **Body → raw → "Examples"** dropdown.
5. The `baseUrl` collection variable defaults to `http://localhost:3000` — leave as-is for local testing.

---

## 4. Test matrix at a glance

| # | Test                                       | Needs real device? | What you check                                              |
|---|--------------------------------------------|--------------------|-------------------------------------------------------------|
| 1 | Smoke test with FAKE token                 | ❌ No              | Server logs show `fcm send failed for token=FAKE...`        |
| 2 | Real device, foreground                    | ✅ Yes             | App in foreground → in-app banner via `flutter_local_notifications` |
| 3 | Real device, background                    | ✅ Yes             | App backgrounded → system tray notification                 |
| 4 | Real device, terminated                    | ✅ Yes             | App killed → push wakes it → tap deep-links to task         |
| 5 | Update transitions                         | ❌ No              | Push fires on `status:completed` and `priority:high`        |
| 6 | Delete triggers push                       | ❌ No              | Server logs show `task_deleted` payload was sent            |
| 7 | Idempotent device registration             | ❌ No              | Second POST returns same `id`, `lastSeenAt` bumped          |
| 8 | Unregister a device                        | ❌ No              | `DELETE /devices/{id}` returns 204; row gone from DB        |

---

## Test 1 — Smoke test with a FAKE token

**Goal:** prove the whole pipeline (Postgres → FCM client → Google) is wired correctly without needing a real Flutter device.

### 1.1 Register a fake device

**Request:** `POST {{baseUrl}}/devices`
**Body (raw JSON):**
```json
{
  "token": "FAKE_TOKEN_FOR_TESTING",
  "platform": "android"
}
```
**Expected response (201 Created):**
```json
{
  "id": "5a2f1e09-...",
  "token": "FAKE_TOKEN_FOR_TESTING",
  "platform": "android",
  "createdAt": "2026-05-23T12:30:00Z",
  "lastSeenAt": "2026-05-23T12:30:00Z"
}
```
✅ **Confirms:** the `devices` table works, the platform enum maps correctly, and the upsert returns the row.

### 1.2 Create a task to trigger a push

**Request:** `POST {{baseUrl}}/tasks`
**Body:**
```json
{
  "title": "Smoke test",
  "description": "Triggers FCM with a bogus token",
  "priority": "high"
}
```
**Expected response (201 Created):** the new task JSON, with a fresh `id` and `createdAt`.

### 1.3 Check the server logs

Switch to your `cargo run` terminal. Within a second of the POST you should see something like:

```
ERROR task_api::handlers: fcm send failed for token=FAKE_TOK: fcm api error 404: { "error": { "code": 404, "message": "Requested entity was not found.", "status": "NOT_FOUND", "details": [...] } }
```

✅ **This error is the success signal.** It means:
- The backend successfully read the device token from Postgres.
- It successfully minted an OAuth2 access token from `service-account.json`.
- It successfully POSTed to `fcm.googleapis.com/v1/projects/flutter-app-fcm-cfd6d/messages:send`.
- Google replied "that token isn't a real registration token" — which is exactly correct.

❌ **If you see something different, jump to [section 13](#13-common-errors-and-what-they-mean).**

---

## Test 2 — Real device, foreground notification

**Goal:** confirm a real Flutter device receives a push **while the app is open**.

### Prerequisites for tests 2–4

- The Flutter app from [`FLUTTER_APP_FCM_INTEGRATION.md`](FLUTTER_APP_FCM_INTEGRATION.md) is installed on an emulator or physical device.
- The app has been launched at least once so it had a chance to call `POST /devices` with its real FCM token.

### 2.1 Verify the real token is in Postgres

In pgAdmin or psql:
```sql
SELECT id, platform, left(token, 30) || '...' AS token_preview, last_seen_at
FROM devices
WHERE token <> 'FAKE_TOKEN_FOR_TESTING'
ORDER BY last_seen_at DESC;
```
You should see your real device with a long token starting like `ep4Cbd4CQN6MaohCViMbxe:APA91b...`.

> Already confirmed in your earlier pgAdmin screenshot — you have one row with token `ep4Cbd4CQN6Maoh...` on platform `android`. Good.

### 2.2 (Optional) Delete the fake token so it doesn't pollute logs

**Request:** `DELETE {{baseUrl}}/devices/{id-of-FAKE_TOKEN-row}`
Expected: **204 No Content**.

### 2.3 Make sure the Flutter app is in the FOREGROUND on the device

The app must be open and visible — not minimized, not killed.

### 2.4 Trigger a push from Postman

**Request:** `POST {{baseUrl}}/tasks`
**Body:**
```json
{
  "title": "Hello from Postman",
  "description": "This should appear as an in-app banner",
  "priority": "medium"
}
```

### 2.5 What you should see

- **On the device** — a notification banner rendered by `flutter_local_notifications` (because FCM does NOT show banners in foreground; the app handles it via `onMessage`).
- **In the server logs** — no error lines about FCM. A successful POST to `fcm.googleapis.com` is silent (no `error!` line is emitted on the happy path).

---

## Test 3 — Real device, background notification

**Goal:** confirm the OS shows a system tray notification when the app is backgrounded.

### 3.1 Background the app

On the device: press **Home** (don't kill it from the app switcher).

### 3.2 Trigger a push

**Request:** `POST {{baseUrl}}/tasks`
**Body:**
```json
{
  "title": "Background test",
  "description": "Should appear in the system tray",
  "priority": "low"
}
```

### 3.3 What you should see

- **On the device** — a system notification in the status bar, using the white `ic_notification` silhouette icon you set up in the Android manifest.
- **Tap the notification** — the app comes to the foreground and (per the deep-link in `data.route`) navigates to `/taskDetails?id=<uuid>`.
- **In server logs** — silent (success).

---

## Test 4 — Real device, terminated state

**Goal:** confirm pushes still arrive when the app has been completely killed.

### 4.1 Kill the app

On the device: open the app switcher and swipe the app away.

### 4.2 Trigger a push

**Request:** `POST {{baseUrl}}/tasks`
**Body:**
```json
{
  "title": "Terminated test",
  "description": "Wakes the app from cold start",
  "priority": "high"
}
```

### 4.3 What you should see

- **On the device** — system tray notification appears even though the app isn't running. This is FCM working through the OS, not your app code.
- **Tap the notification** — the app cold-starts, and on startup Flutter calls `getInitialMessage()` which returns the payload; the router then deep-links to `/taskDetails?id=<uuid>`.

> If the app does NOT cold-start from a tap on Android 12+, the user may have revoked the "notification permission" — check the device settings.

---

## Test 5 — Update triggers

**Goal:** confirm that **only certain** `PUT /tasks/{id}` updates trigger a push.

The broadcast helper only fires when:
- `status` transitions to `completed`, OR
- `priority` transitions to `high`.

A plain rename or description change does NOT push.

### 5.1 Setup — create a baseline task

**Request:** `POST {{baseUrl}}/tasks`
**Body:**
```json
{
  "title": "Update-trigger test",
  "description": "Baseline",
  "priority": "low"
}
```
Note the returned `id` — you'll use it below as `{{taskId}}`. (Tip: in Postman you can set it as a collection variable via the Tests tab: `pm.collectionVariables.set("taskId", pm.response.json().id);`)

### 5.2 Rename only — should NOT trigger a push

**Request:** `PUT {{baseUrl}}/tasks/{{taskId}}`
**Body:**
```json
{
  "title": "Renamed only"
}
```
**Expected:** 200 OK; **no** FCM activity in server logs.

### 5.3 Bump priority to high — SHOULD trigger a push

**Request:** `PUT {{baseUrl}}/tasks/{{taskId}}`
**Body:**
```json
{
  "priority": "high"
}
```
**Expected:** 200 OK; in server logs / on device, a push with:
- title: `Priority bumped`
- body: `<task title>`
- data: `{ "type": "task_priority_high", "taskId": "<uuid>", "route": "/taskDetails" }`

### 5.4 Mark as completed — SHOULD trigger a push

**Request:** `PUT {{baseUrl}}/tasks/{{taskId}}`
**Body:**
```json
{
  "status": "completed"
}
```
**Expected:** 200 OK; the response body now has `completedAt` populated; a push fires with:
- title: `Task completed`
- body: `<task title>`
- data: `{ "type": "task_completed", "taskId": "<uuid>", "route": "/taskDetails" }`

### 5.5 Edge case — bump priority to high on a task that's ALREADY high

**Request:** `PUT {{baseUrl}}/tasks/{{taskId}}`
**Body:**
```json
{
  "priority": "high"
}
```
**Expected:** 200 OK; **no** push (the transition check is `existing_priority != High && new_priority == High`).

---

## Test 6 — Delete triggers a push

**Request:** `DELETE {{baseUrl}}/tasks/{{taskId}}`
**Expected response:** 204 No Content.
**Push payload:**
- title: `Task deleted`
- body: `<deleted task's title>`
- data: `{ "type": "task_deleted", "taskId": "<uuid>", "route": "/taskDetails" }`

> Note: the backend captures the title via `DELETE ... RETURNING title` before the row is gone, so the push body can include the task name.

---

## Test 7 — Idempotent device registration

**Goal:** confirm `POST /devices` with a duplicate token bumps `lastSeenAt` instead of erroring.

### 7.1 First registration

**Request:** `POST {{baseUrl}}/devices`
**Body:**
```json
{
  "token": "duplicate-test-token",
  "platform": "android"
}
```
**Expected:** 201; note the returned `id` and `lastSeenAt`.

### 7.2 Wait a few seconds, then register the SAME token again

Same request, same body. **Expected:**
- 201 again (not 409).
- **Same `id`** as the first call (because of `ON CONFLICT (token) DO UPDATE`).
- **`lastSeenAt` is newer** than the first call.

✅ This is exactly what we want — the Flutter SDK rotates tokens occasionally and re-registers; without an upsert we'd either get duplicates or unique-key errors.

---

## Test 8 — Unregister a device

**Request:** `DELETE {{baseUrl}}/devices/{id}`
(use the `id` from any of the device rows)

**Expected:** 204 No Content. Verify the row is gone:
```sql
SELECT * FROM devices WHERE id = '<the-id-you-deleted>';
```
Should return zero rows.

If you delete a non-existent UUID:
**Expected:** 404 Not Found with body `{"message": "Task not found"}` (the error string reads "Task" because both endpoints share `AppError::NotFound` — purely cosmetic).

---

## 13. Common errors and what they mean

### `fcm api error 404: ... UNREGISTERED`
**Cause:** the device token isn't a real / current FCM registration token.
**Fix:** expected with `FAKE_TOKEN_FOR_TESTING`. For real devices, the Flutter SDK may have rotated the token — reopen the app so it re-registers via `onTokenRefresh`.

### `fcm api error 403: ... PERMISSION_DENIED`
**Cause:** `service-account.json` doesn't have the **Firebase Cloud Messaging API** enabled or the service account lacks the `roles/firebasecloudmessaging.serviceAgent` role.
**Fix:** Firebase Console → Project Settings → Service accounts → regenerate the key. Or check IAM in the Google Cloud Console.

### `fcm api error 400: ... INVALID_ARGUMENT - mismatched project`
**Cause:** the `project_id` inside `service-account.json` doesn't match `FIREBASE_PROJECT_ID` in `.env`.
**Fix:** they must be the same string. For you that's `flutter-app-fcm-cfd6d` in both.

### `oauth error: ... invalid_grant`
**Cause:** the system clock is skewed by more than 5 minutes (JWT `exp` validation fails on Google's side).
**Fix:** sync the system clock (Windows: Settings → Time & language → Date & time → Sync now).

### `broadcast: failed to load device tokens: ...`
**Cause:** the SQL `SELECT token FROM devices` failed — usually because the migration didn't run or the database is unreachable.
**Fix:** verify the `devices` table exists (you've confirmed it does). If not, restart the server so migrations re-run, or apply `migrations/002_create_devices_table.sql` manually.

### POST returns 201 but NO log lines in server about FCM
**Cause:** the `devices` table is empty — the broadcast loop iterates over zero rows.
**Fix:** register at least one device first (Test 1.1).

### App is in foreground but no banner appears
**Cause:** FCM intentionally does not show banners in foreground — your `flutter_local_notifications` config is what renders the in-app banner.
**Fix:** check that `LocalNotificationService.instance.init()` was called from `main.dart` AND that the `task_updates` channel was created.

### Notification icon shows as a white square on Android
**Cause:** the `ic_notification.png` you placed in `android/app/src/main/res/drawable-*` has color in it.
**Fix:** regenerate it as **white-on-transparent silhouette** using <https://romannurik.github.io/AndroidAssetStudio/icons-notification.html>.

---

## 14. Quick reference — all request bodies in one place

### POST /devices

```json
{ "token": "FAKE_TOKEN_FOR_TESTING", "platform": "android" }
```

```json
{ "token": "<real-FCM-token-from-Flutter>", "platform": "android" }
```

```json
{ "token": "<real-FCM-token-from-Flutter>", "platform": "ios" }
```

### DELETE /devices/{id}

No body. Replace `{id}` with a UUID from a prior POST response.

### POST /tasks (triggers `task_created` push)

```json
{
  "title": "My new task",
  "description": "Anything",
  "priority": "high"
}
```

Optional fields:
```json
{
  "title": "Pre-completed task",
  "description": "Starts already done",
  "priority": "medium",
  "status": "completed"
}
```

### PUT /tasks/{id}

Rename only (no push):
```json
{ "title": "Renamed task" }
```

Change description (no push):
```json
{ "description": "Updated description" }
```

Bump priority to high (push fires if previously not high):
```json
{ "priority": "high" }
```

Mark completed (push fires if previously not completed):
```json
{ "status": "completed" }
```

Re-open a completed task (no push, `completedAt` is cleared):
```json
{ "status": "pending" }
```

Combined update (one push fires — `task_completed` wins over priority bump):
```json
{ "status": "completed", "priority": "high", "title": "Final touch" }
```

### DELETE /tasks/{id}

No body. Push fires with the deleted task's title.

### GET /tasks (no push, just list)

No body. Useful to retrieve `id`s for the PUT/DELETE tests above.

### GET /tasks/{id}

No body.

---

**Done.** Once Tests 1, 5, 6, 7, 8 pass (no real device needed), the backend is verified. Tests 2, 3, 4 just confirm the Flutter side once it's wired up per [`FLUTTER_APP_FCM_INTEGRATION.md`](FLUTTER_APP_FCM_INTEGRATION.md).
