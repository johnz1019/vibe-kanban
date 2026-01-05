#!/bin/bash

set -e  # Exit on any error

echo "🧹 Cleaning previous builds..."
rm -rf npx-cli/dist

platform="$(uname -s)"
arch="$(uname -m)"
platform_dir=""

case "${platform}" in
  Linux)
    case "${arch}" in
      x86_64) platform_dir="linux-x64" ;;
      aarch64|arm64) platform_dir="linux-arm64" ;;
    esac
    ;;
  Darwin)
    case "${arch}" in
      x86_64) platform_dir="macos-x64" ;;
      arm64) platform_dir="macos-arm64" ;;
    esac
    ;;
esac

if [[ -z "${platform_dir}" ]]; then
  echo "Unsupported build platform: ${platform}-${arch}"
  echo "Supported: Linux x64/arm64, macOS x64/arm64"
  exit 1
fi

mkdir -p "npx-cli/dist/${platform_dir}"

echo "🔨 Building frontend..."
(cd frontend && npm run build)

echo "🔨 Building Rust binaries..."
cargo build --release --manifest-path Cargo.toml
cargo build --release --bin mcp_task_server --manifest-path Cargo.toml

echo "📦 Creating distribution package..."

# Copy the main binary
cp target/release/server vibe-kanban
zip -q vibe-kanban.zip vibe-kanban
rm -f vibe-kanban 
mv vibe-kanban.zip "npx-cli/dist/${platform_dir}/vibe-kanban.zip"

# Copy the MCP binary
cp target/release/mcp_task_server vibe-kanban-mcp
zip -q vibe-kanban-mcp.zip vibe-kanban-mcp
rm -f vibe-kanban-mcp
mv vibe-kanban-mcp.zip "npx-cli/dist/${platform_dir}/vibe-kanban-mcp.zip"

# Copy the Review CLI binary
cp target/release/review vibe-kanban-review
zip -q vibe-kanban-review.zip vibe-kanban-review
rm -f vibe-kanban-review
mv vibe-kanban-review.zip "npx-cli/dist/${platform_dir}/vibe-kanban-review.zip"

echo "✅ Build complete!"
echo "📁 Files created:"
echo "   - npx-cli/dist/${platform_dir}/vibe-kanban.zip"
echo "   - npx-cli/dist/${platform_dir}/vibe-kanban-mcp.zip"
echo "   - npx-cli/dist/${platform_dir}/vibe-kanban-review.zip"
echo ""
echo "🚀 To test locally, run:"
echo "   cd npx-cli && node bin/cli.js"
