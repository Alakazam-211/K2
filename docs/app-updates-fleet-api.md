# App update feed: the fleet view (`GET /cli/app/updates/fleet`)

K2 0.46.7. An authenticated, **aggregate-only** read of every App's update
feed on one K2 box, for a fleet dashboard (the k2.dev admin portal reads it
alongside the download host's own stats).

## Request

```
GET /cli/app/updates/fleet
Authorization: Bearer <owner token>        (or an Owner/Admin K2 Connect login)
```

- Read-only; `GET` only.
- **Who:** the box's owner token, or a K2 Connect login whose role is Owner
  or Admin. Agent passports, K2 shell tabs, Members, App passes and API keys
  are refused (`403 forbidden`, or the route floor).
- CLI: `k2 app updates fleet [--json]`.
- Apps (beta) off on the box: `404 apps_off`, like every `/cli/app/*` route.

## Answer (`schema: 1`)

```json
{
  "ok": true,
  "schema": 1,
  "box": "rosson",
  "from": "2026-09-30",
  "to": "2026-10-29",
  "apps": [
    {
      "app": "jobs",
      "dlId": "dev.k2.rosson.jobs",
      "channels": [{ "channel": "beta", "version": "1.5.0" }, { "channel": "stable", "version": "1.4.2" }],
      "releases": 6,
      "releasesByState": { "live": 5, "uploading": 0, "failed": 1, "pulled": 0 },
      "files": 14,
      "bytes": 812345678,
      "activeUsers": { "last7Days": 41, "today": 17, "counting": "installs" },
      "checkins": [
        { "day": "2026-10-29", "channel": "stable", "platform": "darwin-aarch64", "version": "1.4.2", "checks": 30, "installs": 12 }
      ]
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `box` | The box's K2 Connect name (the `<box>` of every `dlId`). |
| `from`, `to` | The 30 UTC days `checkins` covers. |
| `apps[].app` | The App id on this box. |
| `apps[].dlId` | Its download id on `dl.k2.dev` (`dev.k2.<box>.<app>`); `null` if the names can't form one. Join key with the download host's stats. |
| `channels` | Each channel and the version it serves now. |
| `releases`, `releasesByState` | Release counts (`pulled` = taken out of every channel). |
| `files`, `bytes` | Uploaded files over all releases and their total size. |
| `activeUsers` | `today`, and `last7Days` = the busiest single day of the last 7. `counting` is `installs` when the App's builds send an anonymous install id (distinct installs per day) or `checks` (update checks per day; a ceiling, one install may check more than once). |
| `checkins[]` | Per UTC day, channel, platform (`<os>-<arch>`, or `-` for the static `latest.json`) and the caller's version (`-` for static): `checks` and new distinct `installs`. |

## What it never contains

No person, username, email, IP address, install id, token, signature or
download URL. K2 keeps no visitor address for update checks at all, and only
a SHA-256 of (App, day, install id) for two days to count distinct installs.

Download counts are the download host's (`GET /api/v1/apps/{id}/stats` with
the stats token): this route doesn't repeat them, so the portal reads each
number from its source.
