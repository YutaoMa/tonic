#!/bin/bash
set -e

# Get the directory where the script is located
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
cd "$SCRIPT_DIR"

echo "Setting up protos in $SCRIPT_DIR/proto..."

mkdir -p proto

TMP_DIR=$(mktemp -d)
echo "Working in $TMP_DIR"

# 1. Envoy
echo "Fetching Envoy protos..."
cd $TMP_DIR
git clone --depth 1 --filter=blob:none --sparse https://github.com/envoyproxy/envoy.git
cd envoy
git sparse-checkout set api/envoy
# Copy api/envoy content (which is the 'envoy' folder structure)
cp -r api/envoy "$SCRIPT_DIR/proto/"
cd ..

# 2. CNCF xDS
echo "Fetching CNCF xDS protos..."
git clone --depth 1 --filter=blob:none --sparse https://github.com/cncf/xds.git cncf_xds
cd cncf_xds
git sparse-checkout set udpa xds
cp -r udpa "$SCRIPT_DIR/proto/"
cp -r xds "$SCRIPT_DIR/proto/"
cd ..

# 3. Validate
echo "Fetching protoc-gen-validate..."
git clone --depth 1 --filter=blob:none --sparse https://github.com/bufbuild/protoc-gen-validate.git
cd protoc-gen-validate
git sparse-checkout set validate
cp -r validate "$SCRIPT_DIR/proto/"
cd ..

# 4. Google APIs
echo "Fetching Google APIs..."
git clone --depth 1 --filter=blob:none --sparse https://github.com/googleapis/googleapis.git
cd googleapis
git sparse-checkout set google/api google/rpc
cp -r google "$SCRIPT_DIR/proto/"
cd ..

rm -rf $TMP_DIR
echo "Done!"

