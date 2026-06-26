Sockdrive v2
============

[![build](https://github.com/caiiiycuk/sockdrive/actions/workflows/build.yml/badge.svg)](https://github.com/caiiiycuk/sockdrive/actions/workflows/build.yml)

Host requirments
================

Install following packages:

```sh
sudo apt install guestfs-tools brotli curl gzip qemu-system-i386 qemu-utils 7zip
```

For working with qcow2 images you must change permission of `/boot/vmlinuz-*`:

```sh
sudo chmod +r /boot/vmlinuz-*
```


This is limitation of `virt-sparsify`, [read more](https://askubuntu.com/questions/1046828/how-to-run-libguestfs-tools-tools-such-as-virt-make-fs-without-sudo)


Binaries
========

Download binaries from [releases](https://github.com/caiiiycuk/sockdrive/releases).


How to use
==========

Follow [js-dos documentation](https://js-dos.com/publish-sockdrive-bundle.html)
