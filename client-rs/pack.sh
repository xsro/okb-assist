cd $(dirname $0)
cargo build --release

mkdir -p target/release2
cp target/release/okb-client.exe target/release2
cp -r ~/.okb/client.json target/release2
cat >target/release2/run.sh << EOF
cd \$(dirname \$0)
mkdir -p ~/.okb/
cp client.json ~/.okb/client.json
./okb-client zotero 我的文库.csv  --dup-doi-action health-hash  --upload-interval 3
EOF

cd target
zip -r "okb-client_$(date "+%Y%m%d%H%M%S").zip" release2
cd -
