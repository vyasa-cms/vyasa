# The official image plus an entrypoint that takes ownership of the volume
# (platforms mount it as root) and drops to the image's user to serve.
FROM ghcr.io/vyasa-cms/vyasa:0.2.0
USER root
COPY entrypoint.sh /usr/local/bin/vyasa-entrypoint
RUN chmod 0755 /usr/local/bin/vyasa-entrypoint
ENTRYPOINT ["vyasa-entrypoint"]
CMD ["serve"]
