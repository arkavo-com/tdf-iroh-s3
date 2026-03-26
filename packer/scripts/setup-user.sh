#!/bin/bash
set -euo pipefail

useradd --system --no-create-home --shell /sbin/nologin tdf-iroh-s3

mkdir -p /etc/tdf-iroh-s3
mkdir -p /var/lib/tdf-iroh-s3

chown tdf-iroh-s3:tdf-iroh-s3 /var/lib/tdf-iroh-s3
chown root:tdf-iroh-s3 /etc/tdf-iroh-s3
chmod 750 /etc/tdf-iroh-s3
chmod 750 /var/lib/tdf-iroh-s3
