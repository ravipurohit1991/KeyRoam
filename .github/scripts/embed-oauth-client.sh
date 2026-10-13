#!/usr/bin/env bash
# Embed the Google OAuth desktop client into the Flutter build via the build
# hook's user-defines. Runs only in CI; the values never get committed.
set -euo pipefail

: "${KEYROAM_GOOGLE_CLIENT_ID:?not set}"

cat >> app/pubspec.yaml <<EOF

hooks:
  user_defines:
    keyroam:
      google_client_id: "${KEYROAM_GOOGLE_CLIENT_ID}"
      google_client_secret: "${KEYROAM_GOOGLE_CLIENT_SECRET:-}"
EOF
echo "Embedded Google OAuth client ${KEYROAM_GOOGLE_CLIENT_ID%%-*}-…"
