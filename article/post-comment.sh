#!/bin/sh
# Posts a dev.to comment from a markdown file.
#   ./post-comment.sh comment-v1.7.md            — top-level comment on the article
#   ./post-comment.sh reply-sinarezaei.md 3goj1  — reply to comment id_code 3goj1
: "${DEVTO_API_KEY:?export DEVTO_API_KEY=... first}"
ARTICLE_ID=4782852   # "~1 MB to orbit" — dev.to/slabb
[ -f "$1" ] || { echo "usage: $0 <comment.md> [reply-to-id-code]"; exit 1; }
cd "$(dirname "$0")"
python3 - "$1" "${2:-}" <<'EOF' > /tmp/navette-comment.json
import json, sys
body = open(sys.argv[1]).read()
payload = {"comment": {"body_markdown": body, "commentable_id": 4782852,
                       "commentable_type": "Article"}}
if sys.argv[2]:
    payload["comment"]["parent_id"] = sys.argv[2]
print(json.dumps(payload))
EOF
curl -s -X POST "https://dev.to/api/comments" \
  -H "Content-Type: application/json" \
  -H "api-key: $DEVTO_API_KEY" \
  -d @/tmp/navette-comment.json | python3 -c \
  "import json,sys; d=json.load(sys.stdin); print('posted:', d.get('url', d))"
rm -f /tmp/navette-comment.json
