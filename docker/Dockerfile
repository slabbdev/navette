# navette container — the browser for agents, no Chromium attached.
# The binary comes from the release pipeline (built natively per arch);
# this image only carries the WebKitGTK runtime stack.
FROM ubuntu:24.04

RUN apt-get update && apt-get install -y --no-install-recommends \
    libwebkit2gtk-4.1-0 \
    libgtk-3-0 \
    glib-networking \
    xvfb \
    dbus \
    fonts-liberation \
    fonts-dejavu-core \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY navette /usr/local/bin/navette
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/navette /usr/local/bin/docker-entrypoint.sh \
    && useradd -m agent

USER agent
ENV PORT=8765
EXPOSE 8765
ENTRYPOINT ["docker-entrypoint.sh"]
# stdio MCP for container-native hosts: docker run -i --rm ghcr.io/slabbdev/navette
CMD ["mcp"]
