#cloud-config
# cloud-init user_data template for a K2 Cloud Standard server.
# The provisioning service substitutes the ${...} placeholders per
# customer and passes the result as Hetzner Cloud `user_data`.
#
# Two deployment shapes:
#   - GOLDEN IMAGE (production): the snapshot already ran
#     `provision-k2-server.sh --bake`, so /opt/k2/provision-k2-server.sh
#     exists and the runcmd below only personalizes (fast path, ~seconds).
#   - RAW UBUNTU (fallback/dev): the script self-fetches everything
#     (slow path, ~2-3 minutes).
#
# PRD: .k2/prds/prd-k2-cloud-hosted-servers-v1.md §2, §4.1.

write_files:
  - path: /etc/k2-provision.env
    permissions: "0600"
    owner: root:root
    content: |
      K2_TUNNEL_TOKEN=${K2_TUNNEL_TOKEN}
      K2_SUBDOMAIN=${K2_SUBDOMAIN}
      K2_OWNER_USER=${K2_OWNER_USER}
      K2_CALLBACK_URL=${K2_CALLBACK_URL}
      K2_CALLBACK_TOKEN=${K2_CALLBACK_TOKEN}

runcmd:
  - |
    set -e
    if [ ! -x /opt/k2/provision-k2-server.sh ]; then
      mkdir -p /opt/k2
      curl -fsSL https://raw.githubusercontent.com/Alakazam-211/K2/main/scripts/provision-k2-server.sh \
        -o /opt/k2/provision-k2-server.sh
      chmod +x /opt/k2/provision-k2-server.sh
    fi
    set -a; . /etc/k2-provision.env; set +a
    /opt/k2/provision-k2-server.sh 2>&1 | tee /var/log/k2-provision.log
    # The generated owner password (if any) is in the log ONLY when no
    # callback URL was provided; the control plane normally receives it
    # via the callback and the log stays credential-free.
    shred -u /etc/k2-provision.env
