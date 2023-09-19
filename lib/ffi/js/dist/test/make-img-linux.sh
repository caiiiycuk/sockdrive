#!/bin/bash
# please install dosfstools


set -ex
# 1Mb
dd if=/dev/zero of=fat12.img bs=1048576 count=1
# 1MB
dd if=/dev/zero of=fat16.img bs=1048576 count=16
# 64Mb
dd if=/dev/zero of=fat32.img bs=1048576 count=64

mkfs.fat -F 12 fat12.img
mkfs.fat -F 16 fat16.img
mkfs.fat -F 32 fat32.img

