#!/usr/bin/env bash
# Embed the Google OAuth desktop client into the Flutter build via the build
# hook's user-defines. Runs only in CI; the values never get committed.
set -euo pipefail

if [ -z "${KEYROAM_GOOGLE_CLIENT_ID:-}" ] && [ -z "${KEYROAM_GOOGLE_MOBILE_CLIENT_ID:-}" ]; then
  echo "No Google OAuth client configured" >&2
  exit 1
fi

# The mobile client (Android) has no secret. Gradle reads the same value for
# the sign-in redirect scheme.
cat >> app/pubspec.yaml <<EOF

hooks:
  user_defines:
    keyroam:
      google_client_id: "${KEYROAM_GOOGLE_CLIENT_ID:-}"
      google_client_secret: "${KEYROAM_GOOGLE_CLIENT_SECRET:-}"
      google_mobile_client_id: "${KEYROAM_GOOGLE_MOBILE_CLIENT_ID:-}"
EOF
echo "Embedded Google OAuth clients: desktop ${KEYROAM_GOOGLE_CLIENT_ID%%-*}…, mobile ${KEYROAM_GOOGLE_MOBILE_CLIENT_ID%%-*}…"
