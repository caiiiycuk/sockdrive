# ffi library for browser

## Building

Compile it with yarn & webpack:

```
yarn
yarn run webpack
```

You can test by opening dist/test.html, but run dist/test/make-img-linux.sh first.

# 3rdparty code

## LZ4

```
/*
MiniLZ4: Minimal LZ4 block decoding and encoding.

based off of node-lz4, https://github.com/pierrec/node-lz4

====
Copyright (c) 2012 Pierre Curto

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
====

changes have the same license
```

## LRU cache

`src/sockdrive/lru.js` is from https://github.com/rsms/js-lru
Licensed under MIT. Copyright (c) 2010 Rasmus Andersson <http://hunch.se/>

## fatfs (fork)

`src/fatfs` is a fork of https://github.com/natevw/fatfs

License information:

```
© 2014 Nathan Vander Wilt.
Funding for this work was provided by Technical Machine, Inc.

Reuse under your choice of:

* BSD-2-Clause
* Apache 2.0
```