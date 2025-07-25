export PATH="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | grep host | cut -d' ' -f2)/bin:$PATH"

mkdir -p profdata
FILES=(target/llvm-cov-target/snapshot_*.profraw)
if [ "${#FILES[@]}" -eq 0 ] || [ "${FILES[0]}" == "snapshot_*.profraw" ]; then
    echo "❌ No snapshot_*.profraw files found!"
    exit 1
fi

echo "✅ Found ${#FILES[@]} snapshot files:"
printf '   - %s\n' "${FILES[@]}"

for f in "${FILES[@]}"; do
    echo "🔍 Processing $f"
    file_name=$(basename "$f")
    llvm-profdata merge -sparse "$f" -o "profdata/${file_name%.profraw}.profdata"
done

FILES=(profdata/snapshot_*.profdata)
mkdir -p jsondata
mkdir -p jsondata/demangled
for FILE in "${FILES[@]}"; do
    echo "🔍 Checking $FILE"
    llvm-cov export "./target/llvm-cov-target/debug/examples/bst" --instr-profile="$FILE" --format=text > jsondata/"$(basename $FILE .profdata)".json
    #  Now run `demangler` to demangle the symbols
    demangler jsondata/"$(basename $FILE .profdata)".json jsondata/demangled/"$(basename $FILE .profdata)".json
done
echo "✅ Exported to jsondata/*.json"