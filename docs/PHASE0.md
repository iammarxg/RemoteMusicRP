# Phase 0: verification spike (2026-11-09)

The research used official documentation wherever it could be reached. Each finding carries a verdict:

- ✅ verified
- ⚠️ partly verified or conditional
- ❌ not possible or not compliant
- **UNVERIFIED** means we could not confirm it from a primary source.

No live API calls were made in this spike.

---

## 1. Discord headless (HTTP) Rich Presence: ❌ no compliant path

**Findings**
- Discord's official documentation has **no** HTTP/REST endpoint for setting a user's presence. The OAuth2 scopes, Social SDK guides, release notes and the docs index (`llms.txt`) were checked. None mentions "headless".
  - https://docs.discord.com/developers/discord-social-sdk/core-concepts/oauth2-scopes
  - https://docs.discord.com/developers/discord-social-sdk/development-guides/setting-rich-presence
  - https://discord.com/developers/docs/social-sdk/release_notes.html
  - https://docs.discord.com/llms.txt
- The official OAuth2 scope table lists these scopes:
  - `activities.write`: "allows your app to update a user's activity", marked **not currently available for apps**.
  - `rpc.activities.write`: available only to approved partners.
  - Source: https://docs.discord.com/developers/topics/oauth2
- `POST /users/@me/headless-sessions` is documented only by the **unofficial** community reference "Discord Userdoccers". The details there:
  - It requires `activities.write`.
  - Sessions expire after 20 minutes.
  - The body has one activity with `application_id` and `platform`.
  - Source: https://docs.discord.food/resources/presence
  - Because the required scope is officially unavailable to apps, using this endpoint would not be compliant.
- Third-party projects that claim "server-side" presence either:
  - use the official Social SDK binary (Natune PR #90, rokku PR #207), or
  - send a reverse-engineered Gateway `IDENTIFY` with an OAuth token (LeonLeBreton/Discord-Social-RPC).
  - The Gateway approach is **not** documented by Discord, so we will not use it.
  - Sources: https://github.com/NatuneGroup/Natune/pull/90, https://github.com/rokku-app/rokku/pull/207, https://github.com/LeonLeBreton/Discord-Social-RPC

**Verdict:** the requested "OAuth2 + HTTP headless sessions" path is **not available** to third-party apps. It is dropped.

### 1b. Official Social SDK running headless: ⚠️ conditional

- The SDK is official. Authenticated presence uses the scopes `openid sdk.social_layer_presence`. A stored token can be reused with `Client::UpdateToken` followed by `Client::Connect`.
  - https://docs.discord.com/developers/discord-social-sdk/development-guides/account-linking-with-discord
- Supported platforms:
  - Windows x64/ARM64 and macOS x64/ARM64 are generally available.
  - **Linux is "Experimental", glibc 2.31+, with no architecture listed.** No Linux arm64 build is listed.
  - https://docs.discord.com/developers/discord-social-sdk/core-concepts/platform-compatibility
- The C++ API is a "header-only wrapper over the C API" in `libdiscord_partner_sdk.so`, so FFI from Rust would be needed (`unsafe`).
  - https://docs.discord.com/developers/discord-social-sdk/getting-started/using-c++
- Rich Presence **without** auth needs a running Discord client.
- **UNVERIFIED:** whether `Client::Connect` and presence work on a headless box with no Discord client.
- A device-authorization flow exists, but it is documented for consoles.
  - https://docs.discord.com/developers/discord-social-sdk/development-guides/account-linking-on-consoles
- Enabling the SDK means filling in a form ("details about your game") and pressing Submit. No review step is described.
- **UNVERIFIED:** whether non-game apps are allowed.
- **UNVERIFIED:** the Social SDK Terms and the Developer ToS/Policy. Every support-dev.discord.com page returned HTTP 403. That includes the redistribution rights for an AGPL project.
- Buttons (up to 2, visible only to other users) and start/end timestamps are supported by the SDK `Activity`.

**Verdict:** this is the only official route to presence with no Discord client on the machine. It needs a hands-on test and a terms review in Phase 1 before we commit to it. Multiple accounts depend on it too.

## 2. Discord local RPC: ✅ supported

- Official: "Rich Presence can be set without authentication" by talking to a running Discord client over RPC, on desktop and on Android (SDK 1.10+).
  - https://docs.discord.com/developers/discord-social-sdk/development-guides/setting-rich-presence
- Note: the older RPC topic page says commands need `AUTHORIZE` and that RPC is limited to approved apps or testers. Common libraries and the Social SDK docs show SET_ACTIVITY working without it. To be confirmed in the Phase 1 live test.
  - https://docs.discord.com/developers/topics/rpc
- **Windows:** named pipe `\\?\pipe\discord-ipc-{0..9}`. Retry on `ERROR_PIPE_BUSY`.
  - https://github.com/discord/discord-rpc/blob/master/src/connection_win.cpp
- **macOS/Linux:** use the first variable that is set out of `XDG_RUNTIME_DIR`, `TMPDIR`, `TMP`, `TEMP`, falling back to `/tmp`. The socket is `${DIR}/discord-ipc-{0..9}`.
  - https://github.com/discord/discord-rpc/blob/master/src/connection_unix.cpp
- **Flatpak** (`$XDG_RUNTIME_DIR/app/com.discordapp.Discord/discord-ipc-N`) and **Snap** (`$XDG_RUNTIME_DIR/snap.discord/discord-ipc-N`): **UNVERIFIED**. These paths are widely used by community clients but not documented by Discord. We will probe them as fallbacks.
- **Framing:** each frame is a u32 LE opcode, then a u32 LE length, then JSON.
  - Opcodes: `0` HANDSHAKE (`{"v":1,"client_id":"…"}`), `1` FRAME, `2` CLOSE, `3` PING, `4` PONG.
  - The client receives `READY` after the handshake.
- **SET_ACTIVITY** takes `args: { pid, activity }`.
  - The activity `type` may be Playing (0), **Listening (2)**, Watching (3) or Competing (5).
  - The documented fields are `state`, `details`, `timestamps {start,end}`, `assets {large_image, large_text, small_image, small_text, *_url}`, `party`, `secrets` and `instance`.
- **UNVERIFIED over local RPC:** `buttons` (the gateway activity object allows at most 2) and `status_display_type` (0 Name, 1 State, 2 Details). Test these live.
  - https://docs.discord.com/developers/events/gateway-events
- The Rust crate `discord-rich-presence` (v1.1.0, MIT) is a reference or a possible dependency. Its maintenance cadence is unverified.
  - https://github.com/vionya/discord-rich-presence

## 3. Last.fm: ✅ usable, with ⚠️ ToS flags

- `user.getRecentTracks`: the current track carries `nowplaying="true"`. `limit` defaults to 50 (max 200), so we use `limit=1`. It does **not** require authentication, only an API key. It has **no** duration field.
  - https://www.last.fm/api/show/user.getRecentTracks
- `track.getInfo`: `duration` is in **milliseconds**. Cache it per track.
  - https://www.last.fm/api/show/track.getInfo
- Write methods need a session; read methods don't.
  - https://www.last.fm/api/authentication
- **Rate limit:** "5 req/s" is **not** an official figure. The official sources say:
  - ToS 4.4: limits are set at Last.fm's "sole discretion".
  - The intro page says an account "may be suspended" for "continuously making several calls per second".
  - Error 29 means the rate limit was exceeded.
  - Also send an identifiable User-Agent.
  - https://www.last.fm/api/tos, https://www.last.fm/api/intro
  - → Our cap is **≤1 req/s per instance**, with a minimum poll interval of 3 s.
- **Caching and storage (ToS 4.3):**
  - Implement caching "in accordance with the HTTP headers".
  - Keep stored data under 100 MB.
  - **UNVERIFIED:** ETag/Last-Modified support.
- **Attribution (ToS 2.7, 4.2.2):**
  - A "powered by AudioScrobbler" credit.
  - Links back to the Last.fm user page or catalogue page.
  - The Brand Guidelines (http://www.last.fm/resources/) were not fetched.
- **Commercial use:** only non-commercial use is allowed (ToS 3.1). Commercial use needs an agreement.
- ⚠️ **ToS 2.7:** "only give public access to pages using Last.fm's web services that have been previously approved by Last.fm in writing". It is unclear whether a Discord presence counts as such a page. **Owner decision needed.**
- ⚠️ **ToS 5.1.8:** artwork available through the API is "expressly excluded" from the agreement. Album art shown on Discord is not licensed through Last.fm. **Owner decision needed.**
- ToS 2.2/2.3 cover registered users and user permission. In our model the end user reads their own data with their own key, which fits the intent. This is a judgement, not a legal opinion.

## 4. ListenBrainz (and others): ✅ API exists, terms ⚠️

- `GET /1/user/{user_name}/playing-now` returns 0 or 1 listen, with `playing_now: true`.
  - No auth header is listed, so it appears to be public. This is not stated explicitly.
  - https://listenbrainz.readthedocs.io/en/latest/users/api/core.html
- The core API page and the JSON doc describe the response envelope differently. Verify live before implementing.
  - https://listenbrainz.readthedocs.io/en/latest/users/json.html
- Duration: `track_metadata.additional_info.duration_ms`, present **only if the submitting client sent it**.
- **Rate limits:** no more than 1 call/s per client app. Responses carry `X-RateLimit-Limit`, `-Remaining` and `-Reset-In` headers, and 429 means the limit was exceeded. A User-Agent is required.
  - https://listenbrainz.readthedocs.io/en/latest/users/api/index.html
- **Terms:** no API terms page could be fetched; listenbrainz.org/terms is client-rendered. The MetaBrainz social contract is non-binding and says content is public-domain or CC-licensed.
  - https://metabrainz.org/social-contract
  - Specific API terms are **UNVERIFIED**.
- Others (e.g. Maloja, Libre.fm) were not evaluated in this spike.

**Verdict:** ListenBrainz is a good second provider behind the same trait. It is not in scope until after v1.

## 5. Platform support matrix (as of 2026-11-09): ✅

| Platform | Supported now | Source |
|---|---|---|
| Windows 11 Home/Pro | 25H2 (to 2027-10-12), 26H2 (to 2028-10-10), 26H1 (new devices only). 24H2 Home/Pro ended 2026-10-13. | https://learn.microsoft.com/en-us/windows/release-health/windows11-release-information |
| Windows 11 Enterprise/Edu | Also 24H2 (to 2027-10-12); LTSC 2024 | same |
| macOS | 27, 26, 15 (Sonoma 14 ended Sep 2026). Apple has no formal policy; it usually supports the latest 3. | https://support.apple.com/en-us/100100, https://endoflife.date/macos |
| Debian | 13 trixie (glibc 2.41), 12 bookworm LTS until 2028-06-30 (glibc 2.36) | https://www.debian.org/releases/, https://wiki.debian.org/LTS, https://packages.debian.org/trixie/libc6 |
| Ubuntu | 22.04, 24.04, 26.04 LTS; 26.10 interim. glibc versions UNVERIFIED. | https://ubuntu.com/about/release-cycle |
| Fedora | 44, 45 (43 ends 2026-12-01). Fedora's own pages blocked automated access; dates are from endoflife.date. | https://endoflife.date/fedora |
| RHEL | 8 (maintenance to 2029), 9, 10. glibc 2.28 for RHEL 8 is UNVERIFIED. | https://access.redhat.com/support/policy/updates/errata |
| Docker | amd64, arm64 (arm/v7 still officially supported by Docker, but out of scope by owner decision) | https://github.com/docker-library/official-images, https://github.com/GoogleContainerTools/distroless |
| 32-bit | Debian 13 and Ubuntu still support armhf; i386 is a co-architecture only in Debian 13 and gone from Ubuntu 20.04+. **Owner decision: no 32-bit builds.** | https://www.debian.org/releases/trixie/ |

**Recommendation:** target a glibc 2.28 floor (e.g. `cargo-zigbuild`) so RHEL 8 works, or ship a static musl binary for the server tarball and packages. Decide in Phase 4.

## 6. Translation platform: ✅ Weblate

- **Hosted Weblate Libre:** free for public libre projects, with the same limits as the 160k-string plan.
  - https://weblate.org/en/hosting/
  - The detailed eligibility page (hosted.weblate.org/hosting) returned 403. **Re-check in Phase 3.**
- **Crowdin OSS:** free, with unlimited projects, strings and members. Requirements:
  - an OSI license
  - public source
  - **at least 3 months of activity**
  - an active community
  - an up-to-date news section
  - regular builds
  - https://crowdin.com/page/open-source-project-setup-request, https://crowdin.com/pricing
- **Recommendation and decision:** Weblate. We don't meet Crowdin's maturity requirements yet, and Weblate is itself open source.

## 7. Album art sources (added 2026-11-09)

**Apple static art via the iTunes Search API: ✅ usable, ⚠️ terms are a grey area**
- No key or account appears in the documentation. Requests look like `https://itunes.apple.com/search?term=…`.
- The limit is "approximately 20 calls per minute (subject to change)".
- `artworkUrl100` is documented at 100×100 (or 60×60). Larger sizes come from changing the URL's size segment (e.g. `1024x1024bb`). This is **undocumented**, so fall back if it fails.
  - https://performance-partners.apple.com/search-api
- Its legal section allows album art "for the purposes of promoting" the content. The art should appear near a store badge that links to Apple, and must not be used "for independent entertainment value".
  - A Discord status is a grey area. We mitigate this with an optional "Listen on Apple Music" button, caching, and no stored image bytes. The owner accepted this on 2026-11-09.

**Apple animated ("motion") art: ❌ no free, compliant path**
- Apple Music API pages are rendered by JavaScript, so the motion-art fields (`editorialVideo`) are **UNVERIFIED** from Apple's docs.
- The API needs a developer token. This is from memory, but consistent with the program page: the Apple Developer Program costs $99/yr, and the free account doesn't list MusicKit.
  - https://developer.apple.com/programs/
- Scraping music.apple.com, or reusing its web-player token, breaks Apple's terms:
  - Apple's website terms forbid "page-scrape", "robot", "spider" or other automatic devices: https://www.apple.com/legal/internet-services/terms/site.html
  - The Apple Media Services Terms forbid automated processes that scrape Content or Services: https://www.apple.com/legal/internet-services/itunes/
- **Owner decision: skipped for now.**

**MusicBrainz Cover Art Archive:** used as the fallback source. It is free and needs no key. MusicBrainz's API limits are 1 req/s and a meaningful User-Agent. These limits are from memory: **re-verify in Phase 1** at https://musicbrainz.org/doc/MusicBrainz_API/Rate_Limiting and https://coverartarchive.org/.

**Discord image support**
- External https URLs are allowed in `assets.large_image`. External URL assets support "any publicly accessible image URL", and GIF, animated WebP and AVIF are accepted for external URLs. Animated *uploaded* assets are not supported.
  - https://docs.discord.com/developers/events/gateway-events (Activity Asset Image)
  - https://docs.discord.com/developers/discord-social-sdk/development-guides/setting-rich-presence
- 1024×1024 is recommended. No size limit is documented for external URLs.
- **UNVERIFIED:**
  - whether external URLs work over **local RPC** SET_ACTIVITY (only the gateway object documents this)
  - whether the client actually *animates* GIF/WebP
  - Discord's caching behaviour
  
  All three get a live test in Phase 1.
- Any locally converted image would need a **publicly reachable** URL. That means exposing the instance, which is one more reason animated art was skipped.

## 8. Recommended defaults → decisions

All open decisions were put to the owner and answered on 2026-11-09. See the Decisions log in `CLAUDE.md`.

Two issues are still open: the Last.fm ToS flags 2.7 and 5.1.8 (§3 above).
