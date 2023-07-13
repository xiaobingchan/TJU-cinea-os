.EXPORT_ALL_VARIABLES:

mode = release

user-rust:
	basename -s .rs src/bin/*.rs | xargs -I {} \
		touch dsk/bin/{}
	basename -s .rs src/bin/*.rs | xargs -I {} \
		cargo rustc --release --bin {}
	basename -s .rs src/bin/*.rs | xargs -I {} \
		cp target/x86_64-cinea_os/release/{} dsk/bin/{}
	if [ `uname -m` = "x86_64" ]; then \
		strip dsk/bin/*; \
	fi


bin = target/x86_64-cinea_os/$(mode)/bootimage-cinea-os.bin
img = disk.img

$(img):
	qemu-img create $(img) 32M

cargo-opts = --release --bin cinea_os
ifeq ($(mode),release)
	cargo-opts += --release
endif

image: $(img)
	touch src/lib.rs
	cargo bootimage
	dd conv=notrunc if=$(bin) of=$(img)