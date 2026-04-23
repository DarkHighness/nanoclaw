FROM debian:bookworm-slim

# The demo builds a local runner image instead of relying on a host-installed
# sysbench binary, so the MySQL/sysbench path remains self-contained.
RUN apt-get update \
 && apt-get install -y --no-install-recommends sysbench default-mysql-client ca-certificates \
 && rm -rf /var/lib/apt/lists/*
