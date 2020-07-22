ghp_branch:='gh-pages'

# Checkout github pages branch into ghp directory
co-ghp:
  mkdir {{ghp_branch}}
  cd ghp
  git init
  git remote add -t {{ghp_branch}} -f origin git@github.com:royaltm/rust-ym-file-parser.git
  git checkout {{ghp_branch}}

# generate documentation
doc: cargo-doc

# generate documentation and udpate github pages directory
update-ghp: doc
  mkdir -p ghp/doc
  rsync -rvah --delete target/doc/ ghp/doc

# generate Rust documentation
cargo-doc:
  cargo +nightly doc --no-deps --lib

test:
  cargo test

clean:
  cargo clean
