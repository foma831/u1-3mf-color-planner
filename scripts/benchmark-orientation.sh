#!/bin/zsh

set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
  print -u2 "Usage: $0 <source.3mf> <object-id> [Snapmaker Orca.app]"
  exit 64
fi

source_path=$1
object_id=$2
orca_app=${3:-/Applications/Snapmaker Orca.app}
orca_binary="$orca_app/Contents/MacOS/Snapmaker_Orca"
repository_root=${0:A:h:h}
converter="$repository_root/target/debug/u1-converter"
benchmark_dir=$(mktemp -d /private/tmp/u1-orientation-benchmark.XXXXXX)
trap 'rm -rf -- "$benchmark_dir"' EXIT

if [[ ! -x $orca_binary ]]; then
  print -u2 "Snapmaker Orca executable was not found at $orca_binary"
  exit 69
fi

cd "$repository_root"
node scripts/run-with-rust.mjs cargo build -p u1-converter-cli --locked >/dev/null

source_hash_before=$(shasum -a 256 "$source_path" | awk '{print $1}')
our_report="$benchmark_dir/our-report.json"
"$converter" optimize-orientation "$source_path" \
  --object-id "$object_id" \
  --instance-id 0 \
  --direction-samples 144 \
  --compact > "$our_report"

# Snapmaker Orca 2.3.5 rejects the newer Bambu project settings before its
# orientation pass. Build a geometry-equivalent, single-object benchmark copy:
# only the primary model entry is streamed out, and the source archive itself
# is never changed or extracted.
orca_input="$benchmark_dir/orca-input.3mf"
cp "$source_path" "$orca_input"
mkdir -p "$benchmark_dir/3D"
unzip -p "$source_path" 3D/3dmodel.model | perl -ne '
  if (/<build\b/) { $build = 1 }
  if (!$build || !/<item\b/ || /objectid="'"$object_id"'"/) {
    s/BambuStudio-02\.06\.00\.51/Snapmaker Orca-01.10.01.50/;
    print;
  }
  if (/<\/build>/) { $build = 0 }
' > "$benchmark_dir/3D/3dmodel.model"
(
  cd "$benchmark_dir"
  zip -q -d orca-input.3mf 'Metadata/*' 'Auxiliaries/*' 'Model/*' >/dev/null 2>&1 || true
  zip -q -u orca-input.3mf 3D/3dmodel.model
)

orca_output="$benchmark_dir/orca-oriented.3mf"
"$orca_binary" --orient 1 --ensure-on-bed --export-3mf "$orca_output" "$orca_input" \
  > "$benchmark_dir/orca.log" 2>&1

# Headless macOS rendering may omit thumbnails while retaining their root
# relationships. Remove only those dangling benchmark relationships so the
# bounded analyzer can score the geometry output.
mkdir -p "$benchmark_dir/_rels"
unzip -p "$orca_output" _rels/.rels | \
  perl -ne 'print unless /plate_1(?:_small)?\.png/' > "$benchmark_dir/_rels/.rels"
(
  cd "$benchmark_dir"
  zip -q -u orca-oriented.3mf _rels/.rels
)

orca_object_id=$("$converter" analyze "$orca_output" --compact | \
  jq -er '.objects | select(length == 1) | .[0].id')
orca_report="$benchmark_dir/orca-report.json"
"$converter" optimize-orientation "$orca_output" \
  --object-id "$orca_object_id" \
  --instance-id 0 \
  --direction-samples 48 \
  --compact > "$orca_report"

source_hash_after=$(shasum -a 256 "$source_path" | awk '{print $1}')
if [[ $source_hash_before != $source_hash_after ]]; then
  print -u2 "Source 3MF changed during the benchmark."
  exit 74
fi

jq -n \
  --arg sourceSha256 "$source_hash_before" \
  --argjson ours "$(jq '.recommendation.metrics' "$our_report")" \
  --argjson orca "$(jq '.source_orientation.metrics' "$orca_report")" \
  '{
    sourceSha256: $sourceSha256,
    ours: $ours,
    orca: $orca,
    scoreImprovement: (1 - ($ours.score / $orca.score)),
    supportVolumeImprovement: (1 - ($ours.estimated_support_volume_mm3 / $orca.estimated_support_volume_mm3)),
    passes: (
      $ours.score < $orca.score and
      $ours.estimated_support_volume_mm3 < $orca.estimated_support_volume_mm3 and
      $ours.small_overhang_component_count <= $orca.small_overhang_component_count
    )
  }' | tee "$benchmark_dir/comparison.json"

jq -e '.passes' "$benchmark_dir/comparison.json" >/dev/null
