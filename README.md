Sockdrive v2
============


[![build](https://github.com/caiiiycuk/sockdrive/actions/workflows/build.yml/badge.svg)](https://github.com/caiiiycuk/sockdrive/actions/workflows/build.yml)

Host requirments
================

Install following packages:

```sh
sudo apt install guestfs-tools brotli qemu-utils
```

For working with qcow2 images you must change permission of `/boot/vmlinuz-*`:

```sh
sudo chmod +r /boot/vmlinuz-*
```


This is limitation of `virt-sparsify`, [read more](https://askubuntu.com/questions/1046828/how-to-run-libguestfs-tools-tools-such-as-virt-make-fs-without-sudo)