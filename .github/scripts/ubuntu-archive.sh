#!/usr/bin/env bash
set -euo pipefail

# Probe official Ubuntu mirrors before apt retries an unreachable archive for minutes.
# Packages still use the image's Ubuntu signing key and normal apt verification.
source /etc/os-release
mirrors=("$@")
if (( ${#mirrors[@]} == 0 )); then
    mirrors=(https://azure.archive.ubuntu.com/ubuntu https://archive.ubuntu.com/ubuntu https://mirrors.kernel.org/ubuntu)
fi
for mirror in "${mirrors[@]}"; do
    # Both suites must work, including the final destination of any redirect.
    for suite in "$VERSION_CODENAME" "$VERSION_CODENAME-security"; do
        if ! curl --location --fail --silent --show-error --connect-timeout 5 --max-time 15 \
            "$mirror/dists/$suite/InRelease" --output "$RUNNER_TEMP/ubuntu-inrelease"; then
            continue 2
        fi
    done

    # Some runner images still give the security suite a separate HTTP address.
    # Point that URI at the probed list without changing its suites or signing key.
    printf '%s/\n' "$mirror" | sudo tee /etc/apt/apt-mirrors.txt
    sudo sed -i -E 's#https?://security\.ubuntu\.com/ubuntu/?([[:space:]]|$)#mirror+file:/etc/apt/apt-mirrors.txt\1#g' \
        /etc/apt/sources.list.d/ubuntu.sources
    sudo apt-get update -qq -o APT::Update::Error-Mode=any -o Acquire::Retries=2 -o Acquire::https::Timeout=30
    exit 0
done

echo 'No Ubuntu archive is reachable.' >&2
exit 1
