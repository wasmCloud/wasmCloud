#!/usr/bin/env bash
# One-time AWS provisioning for the bench pipeline.
#
#   ./scripts/bench/aws/setup-aws.sh \
#     --bucket <bucket> \
#     --region <region> \
#     [--bench-repo  wasmCloud/wasmCloud]
#
# Idempotent. Provisions:
#   1. S3 bucket  -  versioning + AES256 encryption + public access *blocked*
#                    (CloudFront is the only thing that can read it)
#   2. OpenID Connect provider for token.actions.githubusercontent.com
#   3. CloudFront Origin Access Control (OAC) + Distribution serving
#      history.json (cached at the edge, free 1 TB/mo)
#   4. S3 bucket policy: CloudFront-OAC-only read on history.json
#   5. S3 CORS: GET allowed from any origin (data is public via CloudFront)
#   6. WRITE IAM role: trusted by <bench-repo>, may
#      - PutObject to runs/* and history.json
#      - GetObject on history.json (to merge new rows)
#      - CreateInvalidation on the distribution
#
# The site (arewefastyet) reads anonymously through CloudFront, so we no
# longer need a READ role or any AWS auth in the site repo.
#
# Prints the values to set as secrets/vars at the end.
#
# Requires: aws CLI v2, jq.

set -euo pipefail

BUCKET=""
REGION=""
WASMCLOUD_BENCH_REPO="wasmCloud/wasmCloud"
WRITE_ROLE_NAME="wasmcloud-bench-github-oidc-write"
OAC_NAME="wasmcloud-bench-oac"
DIST_COMMENT="arewefastyet bench data"

usage() {
  sed -n '2,/^$/p' "$0" | sed 's/^# \?//'
  exit "${1:-0}"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --bucket)          BUCKET="$2"; shift 2 ;;
    --region)          REGION="$2"; shift 2 ;;
    --bench-repo)      WASMCLOUD_BENCH_REPO="$2"; shift 2 ;;
    --write-role-name) WRITE_ROLE_NAME="$2"; shift 2 ;;
    -h|--help)         usage 0 ;;
    *)                 echo "unknown arg: $1" >&2; usage 2 ;;
  esac
done

[ -n "$BUCKET" ] || { echo "--bucket required"; exit 1; }
[ -n "$REGION" ] || { echo "--region required"; exit 1; }

step() { printf '\n=== %s ===\n' "$*"; }

ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)
OIDC_ARN="arn:aws:iam::${ACCOUNT_ID}:oidc-provider/token.actions.githubusercontent.com"

# 1. Bucket -------------------------------------------------------------------
step "S3 bucket: ${BUCKET} (region ${REGION})"
if aws s3api head-bucket --bucket "$BUCKET" 2>/dev/null; then
  echo "exists"
else
  if [ "$REGION" = "us-east-1" ]; then
    aws s3api create-bucket --bucket "$BUCKET" --region "$REGION"
  else
    aws s3api create-bucket --bucket "$BUCKET" --region "$REGION" \
      --create-bucket-configuration "LocationConstraint=$REGION"
  fi
  echo "created"
fi

aws s3api put-bucket-versioning --bucket "$BUCKET" \
  --versioning-configuration Status=Enabled

aws s3api put-bucket-encryption --bucket "$BUCKET" \
  --server-side-encryption-configuration '{
    "Rules":[{"ApplyServerSideEncryptionByDefault":{"SSEAlgorithm":"AES256"},"BucketKeyEnabled":true}]
  }'

# Block public access fully — CloudFront uses OAC, never anonymous S3 reads.
aws s3api put-public-access-block --bucket "$BUCKET" \
  --public-access-block-configuration \
    BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true

# CORS: harmless because the bucket isn't reachable directly, but lets the
# CloudFront-cached response carry Access-Control-Allow-Origin: * for the site.
aws s3api put-bucket-cors --bucket "$BUCKET" --cors-configuration '{
  "CORSRules":[{"AllowedMethods":["GET"],"AllowedOrigins":["*"],"AllowedHeaders":["*"],"MaxAgeSeconds":3600}]
}'

# 2. OIDC provider ------------------------------------------------------------
step "OIDC provider for github.com"
if aws iam get-open-id-connect-provider --open-id-connect-provider-arn "$OIDC_ARN" >/dev/null 2>&1; then
  echo "exists"
else
  aws iam create-open-id-connect-provider \
    --url https://token.actions.githubusercontent.com \
    --client-id-list sts.amazonaws.com >/dev/null
  echo "created"
fi

# 3. Origin Access Control (OAC) ---------------------------------------------
step "CloudFront Origin Access Control: ${OAC_NAME}"
OAC_ID=$(aws cloudfront list-origin-access-controls \
  --query "OriginAccessControlList.Items[?Name=='${OAC_NAME}'].Id | [0]" \
  --output text)
if [ -z "$OAC_ID" ] || [ "$OAC_ID" = "None" ]; then
  OAC_ID=$(aws cloudfront create-origin-access-control \
    --origin-access-control-config "{
      \"Name\":\"${OAC_NAME}\",
      \"Description\":\"OAC for ${BUCKET} bench data\",
      \"SigningProtocol\":\"sigv4\",
      \"SigningBehavior\":\"always\",
      \"OriginAccessControlOriginType\":\"s3\"
    }" \
    --query 'OriginAccessControl.Id' --output text)
  echo "created: ${OAC_ID}"
else
  echo "exists: ${OAC_ID}"
fi

# 4. CloudFront distribution --------------------------------------------------
# Note on ViewerCertificate.MinimumProtocolVersion below: `TLSv1` looks lax
# but is required when CloudFrontDefaultCertificate is true (we're serving
# from *.cloudfront.net rather than a custom domain). AWS rejects stronger
# minimums in that mode. Bump to TLSv1.2_2021 (or whatever the current
# recommendation is) only after attaching an ACM cert via a custom domain.
step "CloudFront distribution"
DIST_ID=$(aws cloudfront list-distributions \
  --query "DistributionList.Items[?Comment=='${DIST_COMMENT}'].Id | [0]" \
  --output text)
if [ -z "$DIST_ID" ] || [ "$DIST_ID" = "None" ]; then
  # AWS managed cache policy "CachingOptimized" — respects the origin's
  # Cache-Control header, so Cache-Control: max-age=60 on history.json
  # propagates straight through.
  CACHING_OPTIMIZED="658327ea-f89d-4fab-a63d-7e88639e58f6"

  origin_dn="${BUCKET}.s3.${REGION}.amazonaws.com"
  dist_config=$(jq -n \
    --arg origin_dn "$origin_dn" \
    --arg oac "$OAC_ID" \
    --arg ref "bench-$(date -u +%s)" \
    --arg comment "$DIST_COMMENT" \
    --arg cache_policy "$CACHING_OPTIMIZED" \
    '{
      CallerReference: $ref,
      Comment: $comment,
      Enabled: true,
      PriceClass: "PriceClass_100",
      HttpVersion: "http2and3",
      IsIPV6Enabled: true,
      Origins: {
        Quantity: 1,
        Items: [{
          Id: "s3-origin",
          DomainName: $origin_dn,
          OriginPath: "",
          CustomHeaders: {Quantity: 0},
          S3OriginConfig: {OriginAccessIdentity: ""},
          OriginAccessControlId: $oac,
          ConnectionAttempts: 3,
          ConnectionTimeout: 10
        }]
      },
      DefaultCacheBehavior: {
        TargetOriginId: "s3-origin",
        ViewerProtocolPolicy: "redirect-to-https",
        AllowedMethods: {
          Quantity: 2, Items: ["GET", "HEAD"],
          CachedMethods: {Quantity: 2, Items: ["GET", "HEAD"]}
        },
        Compress: true,
        CachePolicyId: $cache_policy,
        SmoothStreaming: false,
        FieldLevelEncryptionId: ""
      },
      ViewerCertificate: {
        CloudFrontDefaultCertificate: true,
        MinimumProtocolVersion: "TLSv1",
        SSLSupportMethod: "vip"
      },
      Restrictions: {GeoRestriction: {RestrictionType: "none", Quantity: 0}},
      DefaultRootObject: "",
      WebACLId: ""
    }')

  DIST_ID=$(aws cloudfront create-distribution \
    --distribution-config "$dist_config" \
    --query 'Distribution.Id' --output text)
  echo "created: ${DIST_ID} (still propagating; takes 5-15 min before first hit)"
else
  echo "exists: ${DIST_ID}"
fi

DIST_DOMAIN=$(aws cloudfront get-distribution \
  --id "$DIST_ID" --query 'Distribution.DomainName' --output text)
DIST_ARN="arn:aws:cloudfront::${ACCOUNT_ID}:distribution/${DIST_ID}"

# 5. Bucket policy: only this distribution can GetObject history.json --------
step "S3 bucket policy: CloudFront-OAC only on history.json"
bucket_policy=$(jq -n \
  --arg b "$BUCKET" \
  --arg dist "$DIST_ARN" \
  '{
    Version: "2012-10-17",
    Statement: [{
      Sid: "AllowCloudFrontServicePrincipalReadHistory",
      Effect: "Allow",
      Principal: {Service: "cloudfront.amazonaws.com"},
      Action: "s3:GetObject",
      Resource: "arn:aws:s3:::\($b)/history.json",
      Condition: {StringEquals: {"AWS:SourceArn": $dist}}
    }]
  }')
aws s3api put-bucket-policy --bucket "$BUCKET" --policy "$bucket_policy"

# Helper: create or reconcile a role with the given trust + inline policy.
upsert_role() {
  local role_name="$1"
  local trust="$2"
  local policy="$3"
  local policy_name="${role_name}-policy"

  if aws iam get-role --role-name "$role_name" >/dev/null 2>&1; then
    aws iam update-assume-role-policy --role-name "$role_name" --policy-document "$trust"
    echo "  trust policy updated"
  else
    aws iam create-role --role-name "$role_name" --assume-role-policy-document "$trust" >/dev/null
    echo "  created"
  fi
  aws iam put-role-policy --role-name "$role_name" \
    --policy-name "$policy_name" --policy-document "$policy"
}

# 6. WRITE role: bench pipeline ----------------------------------------------
step "WRITE role: ${WRITE_ROLE_NAME}  (trusted by ${WASMCLOUD_BENCH_REPO})"
write_trust=$(jq -n \
  --arg arn "$OIDC_ARN" \
  --arg sub "repo:${WASMCLOUD_BENCH_REPO}:*" \
  '{
    Version: "2012-10-17",
    Statement: [{
      Effect: "Allow",
      Principal: {Federated: $arn},
      Action: "sts:AssumeRoleWithWebIdentity",
      Condition: {
        StringEquals: {"token.actions.githubusercontent.com:aud": "sts.amazonaws.com"},
        StringLike:   {"token.actions.githubusercontent.com:sub": $sub}
      }
    }]
  }')
write_policy=$(jq -n --arg b "$BUCKET" --arg dist "$DIST_ARN" '{
  Version: "2012-10-17",
  Statement: [
    { Sid: "WriteRunArtifacts",
      Effect: "Allow",
      Action: ["s3:PutObject","s3:PutObjectAcl","s3:AbortMultipartUpload"],
      Resource: "arn:aws:s3:::\($b)/runs/*" },
    { Sid: "ReadWriteHistoryAggregate",
      Effect: "Allow",
      Action: ["s3:GetObject","s3:PutObject"],
      Resource: "arn:aws:s3:::\($b)/history.json" },
    { Sid: "ListBucketForPrefixDiscovery",
      Effect: "Allow",
      Action: ["s3:ListBucket"],
      Resource: "arn:aws:s3:::\($b)" },
    { Sid: "InvalidateCloudFrontHistory",
      Effect: "Allow",
      Action: ["cloudfront:CreateInvalidation"],
      Resource: $dist }
  ]
}')
upsert_role "$WRITE_ROLE_NAME" "$write_trust" "$write_policy"
WRITE_ARN=$(aws iam get-role --role-name "$WRITE_ROLE_NAME" --query 'Role.Arn' --output text)

# 7. Print configuration ------------------------------------------------------
DATA_URL="https://${DIST_DOMAIN}/history.json"
cat <<EOM

=== done ===

Set the following on each repo:

  ${WASMCLOUD_BENCH_REPO}    (bench pipeline — writes to S3, invalidates CloudFront)
    secret  WASMCLOUD_BENCH_AWS_ROLE_ARN          = ${WRITE_ARN}
    secret  WASMCLOUD_BENCH_S3_BUCKET             = ${BUCKET}
    secret  WASMCLOUD_BENCH_S3_REGION             = ${REGION}
    secret  WASMCLOUD_BENCH_CF_DISTRIBUTION_ID    = ${DIST_ID}

  wasmCloud/arewefastyet    (trend site — anonymous reads via CloudFront)
    var     DATA_URL                    = ${DATA_URL}

Distribution domain:
  ${DIST_DOMAIN}    (use this URL until/unless you attach a custom domain)

If the distribution was just created, it takes ~5-15 minutes for the
edge configuration to propagate. After that, push a bench run; the
site should pick it up within \`max-age=60\`.
EOM
