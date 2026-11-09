# SPDX-License-Identifier: AGPL-3.0-only
#
# RemoteMusicRP container images. Nothing is compiled here: the images
# are built from the signed release binaries, which the docker workflow
# downloads and verifies (minisign + SHA256SUMS) first. The build context
# must contain:
#   bin/remotemusicrp-amd64   bin/remotemusicrp-arm64
#   remotemusicrp.toml        (this folder's copy)
#
# Targets:
#   sdk      (amd64 only) Debian slim + the audio/X11 client libraries the
#            Discord Social SDK links against, for the experimental server
#            mode. The SDK itself is NEVER included (its terms forbid
#            redistribution): mount your own copy read-only at /sdk.
#   default  (amd64 + arm64) distroless, no shell. Without the SDK the only
#            Discord mode is local RPC, which needs a Discord app on the same
#            machine, so this image is mainly a base for future providers.

# debian:13-slim, pinned by digest.
FROM debian:13-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2 AS sdk
ARG TARGETARCH
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libatomic1 libasound2t64 libpulse0 libx11-6 \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /data /keys /sdk /etc/remotemusicrp \
 && chown 65532:65532 /data /keys \
 && chmod 0700 /data /keys
COPY --chmod=0444 remotemusicrp.toml /etc/remotemusicrp/remotemusicrp.toml
COPY --chmod=0555 bin/remotemusicrp-${TARGETARCH} /usr/local/bin/remotemusicrp
ENV REMOTEMUSICRP_CONFIG=/etc/remotemusicrp/remotemusicrp.toml \
    REMOTEMUSICRP_INSTALL_KIND=docker \
    REMOTEMUSICRP_DISCORD_MODE=social-sdk \
    REMOTEMUSICRP_DISCORD_SDK_LIBRARY=/sdk/libdiscord_partner_sdk.so
VOLUME ["/data", "/keys"]
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/remotemusicrp"]
CMD ["run"]

# gcr.io/distroless/cc-debian13:nonroot, pinned by digest.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cc66d222f0a7ec996b78a99fe76a969bc80b539a5016ad866c14d1949bc6d0ab AS default
ARG TARGETARCH
COPY --chmod=0444 remotemusicrp.toml /etc/remotemusicrp/remotemusicrp.toml
COPY --chmod=0555 bin/remotemusicrp-${TARGETARCH} /usr/local/bin/remotemusicrp
COPY --from=sdk --chown=65532:65532 /data /data
COPY --from=sdk --chown=65532:65532 /keys /keys
ENV REMOTEMUSICRP_CONFIG=/etc/remotemusicrp/remotemusicrp.toml \
    REMOTEMUSICRP_INSTALL_KIND=docker
VOLUME ["/data", "/keys"]
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/remotemusicrp"]
CMD ["run"]
