#!/bin/sh
# Creates the `lakehouse` catalog in Polaris (once) and lets the root principal manage its
# content. Pattern from Polaris's own getting-started guides (1.8.0), without credential vending:
# SeaweedFS has no STS, so `stsUnavailable` is set and clients use static keys.
set -eu
P=http://lh-polaris:8181
H="Polaris-Realm: POLARIS"

TOKEN=$(curl -sf "$P/api/catalog/v1/oauth/tokens" -H "$H" \
  --user "$CLIENT_ID:$CLIENT_SECRET" \
  -d grant_type=client_credentials -d scope=PRINCIPAL_ROLE:ALL \
  | sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')
[ -n "$TOKEN" ] || { echo "no token from Polaris"; exit 1; }

if curl -sf -o /dev/null "$P/api/management/v1/catalogs/lakehouse" -H "$H" \
     -H "Authorization: Bearer $TOKEN"; then
  echo "catalog lakehouse exists"
  exit 0
fi

curl -sf "$P/api/management/v1/catalogs" -H "$H" -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{
  "catalog": {
    "name": "lakehouse",
    "type": "INTERNAL",
    "readOnly": false,
    "properties": { "default-base-location": "s3://lakehouse/warehouse" },
    "storageConfigInfo": {
      "storageType": "S3",
      "allowedLocations": ["s3://lakehouse/warehouse"],
      "endpoint": "http://lh-seaweedfs:8333",
      "pathStyleAccess": true,
      "stsUnavailable": true,
      "region": "us-east-1"
    }
  }
}'

curl -sf -X PUT \
  "$P/api/management/v1/catalogs/lakehouse/catalog-roles/catalog_admin/grants" \
  -H "$H" -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"type":"catalog","privilege":"CATALOG_MANAGE_CONTENT"}'
echo "catalog lakehouse created"
