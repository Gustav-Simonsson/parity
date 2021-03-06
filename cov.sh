#!/bin/sh
# Installing KCOV under ubuntu
# https://users.rust-lang.org/t/tutorial-how-to-collect-test-coverages-for-rust-project/650#
### Install deps
# $ sudo apt-get install libcurl4-openssl-dev libelf-dev libdw-dev cmake gcc binutils-dev libiberty-dev
#
### Compile kcov
# $ wget https://github.com/SimonKagstrom/kcov/archive/master.tar.gz && tar xf master.tar.gz
# $ cd kcov-master && mkdir build && cd build
# $ cmake .. && make && sudo make install

### Running coverage
if ! type kcov > /dev/null; then
   	echo "Install kcov first (details inside this file). Aborting."
	exit 1
fi

cargo test --features ethcore/json-tests -p ethcore --no-run || exit $?
mkdir -p target/coverage
kcov --exclude-pattern ~/.multirust,rocksdb,secp256k1 --include-pattern src --verify target/coverage target/debug/deps/ethcore*
xdg-open target/coverage/index.html
