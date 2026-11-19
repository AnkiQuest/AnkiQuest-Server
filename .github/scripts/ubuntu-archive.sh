#!/usr/bin/env bash
set -euo pipefail

# Probe official Ubuntu mirrors before apt retries an unreachable archive for minutes.
# Packages still use the image's Ubuntu signing key and normal apt verification.
source /etc/os-release
mirrors=("$@")
if (( ${#mirrors[@]} == 0 )); then
    mirrors=(https://azure.archive.ubuntu.com/ubuntu https://archive.ubuntu.com/ubuntu https://mirrors.kernel.org/ubuntu)
fi
reachable=()
for mirror in "${mirrors[@]}"; do
    # Both suites must work, including the final destination of any redirect.
    for suite in "$VERSION_CODENAME" "$VERSION_CODENAME-security"; do
        if ! curl --location --fail --silent --show-error --connect-timeout 5 --max-time 15 \
            "$mirror/dists/$suite/InRelease" --output "$RUNNER_TEMP/ubuntu-inrelease"; then
            continue 2
        fi
    done

    reachable+=("$mirror")
done

if (( ${#reachable[@]} == 0 )); then
    echo 'No Ubuntu archive is reachable.' >&2
    exit 1
fi

# Keep every working mirror: apt can try another if a package download fails.
# Some runner images still give the security suite a separate HTTP address.
# Change that URI without changing the suites or Ubuntu signing key.
printf '%s/\n' "${reachable[@]}" | sudo tee /etc/apt/apt-mirrors.txt
sudo sed -i -E 's#https?://security\.ubuntu\.com/ubuntu/?([[:space:]]|$)#mirror+file:/etc/apt/apt-mirrors.txt\1#g' \
    /etc/apt/sources.list.d/ubuntu.sources
sudo apt-get update -qq -o APT::Update::Error-Mode=any -o Acquire::Retries=2 -o Acquire::https::Timeout=30
