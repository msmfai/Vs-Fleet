#!/usr/bin/env bash
# Build the Fleet bridge extension and package a minimal VSIX from source.
#
# This intentionally avoids a network-time dependency on `vsce`: Fleet's app
# bundler needs the bridge artifact to match the checked-out source, and the
# extension is small enough that a minimal local zip is sufficient here.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
cd "$HERE"

npm run build

NAME="$(node -p "require('./package.json').name")"
DISPLAY_NAME="$(node -p "require('./package.json').displayName")"
DESCRIPTION="$(node -p "require('./package.json').description")"
VERSION="$(node -p "require('./package.json').version")"
PUBLISHER="$(node -p "require('./package.json').publisher")"
ENGINE="$(node -p "require('./package.json').engines.vscode")"
EXTENSION_KIND="$(node -p "(require('./package.json').extensionKind || []).join(',')")"
OUT="$HERE/${NAME}-${VERSION}.vsix"
STAGE="$(mktemp -d)"

cleanup() {
  rm -rf "$STAGE"
}
trap cleanup EXIT

mkdir -p "$STAGE/extension"
cp package.json tsconfig.json .gitignore "$STAGE/extension/"
cp -R src out "$STAGE/extension/"
mkdir -p "$STAGE/extension/node_modules"
cp -R node_modules/ws "$STAGE/extension/node_modules/"

cat > "$STAGE/extension.vsixmanifest" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011" xmlns:d="http://schemas.microsoft.com/developer/vsx-schema-design/2011">
  <Metadata>
    <Identity Language="en-US" Id="$NAME" Version="$VERSION" Publisher="$PUBLISHER" />
    <DisplayName>$DISPLAY_NAME</DisplayName>
    <Description xml:space="preserve">$DESCRIPTION</Description>
    <Tags></Tags>
    <Categories>Other</Categories>
    <GalleryFlags>Public</GalleryFlags>
    <Properties>
      <Property Id="Microsoft.VisualStudio.Code.Engine" Value="$ENGINE" />
      <Property Id="Microsoft.VisualStudio.Code.ExtensionDependencies" Value="" />
      <Property Id="Microsoft.VisualStudio.Code.ExtensionPack" Value="" />
      <Property Id="Microsoft.VisualStudio.Code.ExtensionKind" Value="$EXTENSION_KIND" />
      <Property Id="Microsoft.VisualStudio.Code.LocalizedLanguages" Value="" />
      <Property Id="Microsoft.VisualStudio.Code.EnabledApiProposals" Value="" />
      <Property Id="Microsoft.VisualStudio.Code.ExecutesCode" Value="true" />
      <Property Id="Microsoft.VisualStudio.Services.GitHubFlavoredMarkdown" Value="true" />
      <Property Id="Microsoft.VisualStudio.Services.Content.Pricing" Value="Free"/>
    </Properties>
  </Metadata>
  <Installation>
    <InstallationTarget Id="Microsoft.VisualStudio.Code"/>
  </Installation>
  <Dependencies/>
  <Assets>
    <Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true" />
  </Assets>
</PackageManifest>
EOF

cat > "$STAGE/[Content_Types].xml" <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="json" ContentType="application/json"/>
  <Default Extension="js" ContentType="application/javascript"/>
  <Default Extension="ts" ContentType="application/typescript"/>
  <Default Extension="mjs" ContentType="application/javascript"/>
  <Default Extension="md" ContentType="text/markdown"/>
  <Default Extension="txt" ContentType="text/plain"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/extension.vsixmanifest" ContentType="text/xml"/>
</Types>
EOF

rm -f "$OUT"
(cd "$STAGE" && zip -qr "$OUT" .)
echo "built $OUT"
