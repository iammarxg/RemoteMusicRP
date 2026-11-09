// SPDX-License-Identifier: AGPL-3.0-only
//
// Phase 1 spike: can the official Discord Social SDK set Rich Presence on a
// machine with NO Discord client and NO GUI (e.g. a headless Linux server)?
//
// Flow (all official SDK APIs):
//   1. PKCE: CreateAuthorizationCodeVerifier -> print the authorize URL.
//   2. The owner opens the URL in their own browser, approves, and pastes
//      the `code` from the redirect URL back here.
//   3. GetToken (public client) -> UpdateToken(Bearer) -> Connect.
//   4. When Ready: UpdateRichPresence (Listening, art URL, timestamps,
//      2 buttons) and keep running so the result can be observed.
//
// The access token only lives in memory; nothing is written to disk.
// This is throwaway test code, not part of the product.

#define DISCORDPP_IMPLEMENTATION
#include "discordpp.h"

#include <atomic>
#include <chrono>
#include <csignal>
#include <cstdlib>
#include <ctime>
#include <iostream>
#include <string>
#include <thread>

static std::atomic<bool> g_stop{false};

static const char* kRedirectUri = "http://127.0.0.1/callback";

static std::string url_encode(const std::string& s) {
    static const char* hex = "0123456789ABCDEF";
    std::string out;
    for (unsigned char c : s) {
        if (isalnum(c) || c == '-' || c == '_' || c == '.' || c == '~') {
            out += static_cast<char>(c);
        } else {
            out += '%';
            out += hex[c >> 4];
            out += hex[c & 15];
        }
    }
    return out;
}

static void set_presence(discordpp::Client& client, const std::string& image) {
    discordpp::Activity a;
    a.SetType(discordpp::ActivityTypes::Listening);
    a.SetDetails("Headless SDK test");
    a.SetState("by RemoteMusicRP spike");
    a.SetStatusDisplayType(discordpp::StatusDisplayTypes::Details);

    discordpp::ActivityAssets assets;
    assets.SetLargeImage(image);
    assets.SetLargeText("External image URL test");
    a.SetAssets(assets);

    auto now = static_cast<uint64_t>(std::time(nullptr));
    discordpp::ActivityTimestamps ts;
    ts.SetStart(now);
    ts.SetEnd(now + 180);
    a.SetTimestamps(ts);

    discordpp::ActivityButton b1;
    b1.SetLabel("View on Last.fm");
    b1.SetUrl("https://www.last.fm/");
    a.AddButton(b1);
    discordpp::ActivityButton b2;
    b2.SetLabel("Get this status");
    b2.SetUrl("https://github.com/iammarxg/RemoteMusicRP");
    a.AddButton(b2);

    client.UpdateRichPresence(a, [](discordpp::ClientResult r) {
        if (r.Successful()) {
            std::cout << "[RESULT] UpdateRichPresence: SUCCESS" << std::endl;
        } else {
            std::cout << "[RESULT] UpdateRichPresence: FAILED: " << r.Error() << std::endl;
        }
    });
}

int main(int argc, char** argv) {
    if (argc < 2) {
        std::cerr << "usage: " << argv[0] << " <application_id> [seconds_to_run=300] [image_url]\n";
        return 2;
    }
    const uint64_t app_id = std::strtoull(argv[1], nullptr, 10);
    const int run_secs = argc > 2 ? std::atoi(argv[2]) : 300;
    const std::string image =
        argc > 3 ? argv[3] : "https://avatars.githubusercontent.com/u/9919?s=512";

    std::signal(SIGINT, [](int) { g_stop = true; });
    std::signal(SIGTERM, [](int) { g_stop = true; });

    auto client = std::make_shared<discordpp::Client>();
    client->AddLogCallback(
        [](std::string msg, discordpp::LoggingSeverity sev) {
            std::cout << "[SDK " << static_cast<int>(sev) << "] " << msg;
            if (msg.empty() || msg.back() != '\n') std::cout << std::endl;
        },
        discordpp::LoggingSeverity::Warning);

    std::atomic<bool> presence_sent{false};
    client->SetStatusChangedCallback([client, image, &presence_sent](discordpp::Client::Status s,
                                                                     discordpp::Client::Error e,
                                                                     int32_t detail) {
        std::cout << "[STATUS] " << discordpp::Client::StatusToString(s);
        if (e != discordpp::Client::Error::None) {
            std::cout << " error=" << discordpp::Client::ErrorToString(e) << " detail=" << detail;
        }
        std::cout << std::endl;
        if (s == discordpp::Client::Status::Ready && !presence_sent.exchange(true)) {
            std::cout << "[INFO] Ready without a Discord client. Setting presence..." << std::endl;
            set_presence(*client, image);
        }
    });

    auto verifier = client->CreateAuthorizationCodeVerifier();
    std::string url = std::string("https://discord.com/oauth2/authorize?response_type=code") +
                      "&client_id=" + std::to_string(app_id) +
                      "&redirect_uri=" + url_encode(kRedirectUri) +
                      "&scope=" + url_encode("openid sdk.social_layer_presence") +
                      "&code_challenge=" + url_encode(verifier.Challenge().Challenge()) +
                      "&code_challenge_method=S256";

    std::cout << "\n1) Open this URL in your browser and click Authorize:\n\n" << url << "\n\n"
              << "2) The browser then goes to http://127.0.0.1/callback?code=... (the page will\n"
              << "   fail to load; that's expected). Copy the value after code= and paste it here.\n\n"
              << "code: " << std::flush;

    std::string code;
    std::getline(std::cin, code);
    while (!code.empty() && (code.back() == '\r' || code.back() == ' ')) code.pop_back();
    if (code.empty()) {
        std::cerr << "no code entered\n";
        return 1;
    }

    std::atomic<bool> token_done{false};
    client->GetToken(app_id, code, verifier.Verifier(), kRedirectUri,
                     [client, &token_done](discordpp::ClientResult r, std::string access,
                                           std::string /*refresh*/, discordpp::AuthorizationTokenType type,
                                           int32_t expires_in, std::string scopes) {
                         if (!r.Successful()) {
                             std::cout << "[RESULT] GetToken FAILED: " << r.Error() << std::endl;
                             g_stop = true;
                             return;
                         }
                         std::cout << "[RESULT] GetToken OK (expires_in=" << expires_in
                                   << "s, scopes=" << scopes << ")" << std::endl;
                         client->UpdateToken(type, access, [client](discordpp::ClientResult r2) {
                             if (r2.Successful()) {
                                 std::cout << "[INFO] token set; connecting..." << std::endl;
                                 client->Connect();
                             } else {
                                 std::cout << "[RESULT] UpdateToken FAILED: " << r2.Error() << std::endl;
                                 g_stop = true;
                             }
                         });
                         token_done = true;
                     });

    auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(run_secs);
    while (!g_stop && std::chrono::steady_clock::now() < deadline) {
        discordpp::RunCallbacks();
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }
    std::cout << "[INFO] exiting (presence is NOT cleared, to observe what happens after exit)"
              << std::endl;
    return 0;
}
