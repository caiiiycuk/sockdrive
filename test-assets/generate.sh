# 256Mb
dd if=/dev/zero of=test-assets/fat16-256mb.raw bs=1024 count=246456

# 256Mb
dd if=/dev/zero of=test-assets/fat32-2gb.raw bs=1024 count=2097152

mkfs.fat -F 16 test-assets/fat16-256mb.raw
mkfs.fat -F 32 test-assets/fat32-2gb.raw