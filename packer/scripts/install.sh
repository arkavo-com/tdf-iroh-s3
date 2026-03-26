#!/bin/bash
set -euo pipefail

install -m 755 /tmp/tdf-iroh-s3 /usr/local/bin/tdf-iroh-s3
install -m 755 /tmp/bootstrap.sh /usr/local/bin/tdf-iroh-s3-bootstrap
install -m 644 /tmp/tdf-iroh-s3.service /etc/systemd/system/tdf-iroh-s3.service

systemctl daemon-reload
systemctl enable tdf-iroh-s3.service
