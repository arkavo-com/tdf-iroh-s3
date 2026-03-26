#!/bin/bash
set -euo pipefail

CONFIG_PATH="/etc/tdf-iroh-s3/config.toml"

if [ -f "$CONFIG_PATH" ]; then
    echo "Config exists at $CONFIG_PATH"
    exit 0
fi

echo "No config found, checking instance user-data..."

TOKEN=$(curl -s -X PUT "http://169.254.169.254/latest/api/token" \
    -H "X-aws-ec2-metadata-token-ttl-seconds: 60" 2>/dev/null || true)

if [ -n "$TOKEN" ]; then
    USER_DATA=$(curl -s -H "X-aws-ec2-metadata-token: $TOKEN" \
        "http://169.254.169.254/latest/user-data" 2>/dev/null || true)

    if [ -n "$USER_DATA" ] && echo "$USER_DATA" | head -1 | grep -q '^\['; then
        echo "Found TOML config in user-data, writing to $CONFIG_PATH"
        echo "$USER_DATA" > "$CONFIG_PATH"
        chown tdf-iroh-s3:tdf-iroh-s3 "$CONFIG_PATH"
        chmod 640 "$CONFIG_PATH"
        exit 0
    fi
fi

echo "ERROR: No config file and no valid user-data found"
exit 1
