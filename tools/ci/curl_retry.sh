#!/usr/bin/env bash
# Download a URL to DEST, retrying transient curl failures (TLS reset, peer
# reset, 5xx). GitHub release assets (buildifier) occasionally fail with
#   curl: (35) Recv failure: Connection reset by peer
# on ubuntu-latest; a single -fsSL is not enough.
#
# Usage: curl_retry.sh DEST URL
# Env: CURL_RETRY_ATTEMPTS (default 5), CURL_RETRY_DELAY (default 2 seconds;
#      doubles after each failed attempt).
#
# Successful curl keeps -fsSL semantics (-f fail on HTTP errors, -S show
# errors, -s silent, -L follow redirects). Partial files are discarded.
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 DEST URL" >&2
  exit 2
fi

dest=$1
url=$2
attempts=${CURL_RETRY_ATTEMPTS:-5}
delay=${CURL_RETRY_DELAY:-2}

if ! [[ "$attempts" =~ ^[1-9][0-9]*$ ]]; then
  echo "curl_retry: CURL_RETRY_ATTEMPTS must be a positive integer" >&2
  exit 2
fi

tmp="${dest}.tmp.$$"
cleanup() { rm -f "$tmp"; }
trap cleanup EXIT

attempt=1
while true; do
  status=0
  curl -fsSL -o "$tmp" "$url" || status=$?
  if [[ "$status" -eq 0 && -s "$tmp" ]]; then
    mv -f "$tmp" "$dest"
    trap - EXIT
    exit 0
  fi
  rm -f "$tmp"
  if [[ "$attempt" -ge "$attempts" ]]; then
    echo "curl_retry: failed after ${attempts} attempts (last curl exit ${status}) fetching ${url}" >&2
    exit "${status:-1}"
  fi
  echo "curl_retry: attempt ${attempt}/${attempts} failed (curl exit ${status}); retrying in ${delay}s" >&2
  sleep "$delay"
  delay=$((delay * 2))
  attempt=$((attempt + 1))
done
