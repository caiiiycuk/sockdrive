# sockdrive client api sdk

# js

Plain js api ready to use in browser.

Building:

```js
cd js
yarn
yarn run webpack
```

Js api is in `js/dist/sockdriveFat.js`. The api is node-like fs api.

Testing:

1. Install dosfstools
2. cd js/dist/test && make-img-linux.sh
3. Serve dist folder
4. Open test.html in browser to run tests.

## 3rdparty code

### LRU cache

`src/sockdrive/lru.js` is from https://github.com/rsms/js-lru
Licensed under MIT. Copyright (c) 2010 Rasmus Andersson <http://hunch.se/>

### fatfs (fork)

`src/fatfs` is a fork of https://github.com/natevw/fatfs

License information:

```
© 2014 Nathan Vander Wilt.
Funding for this work was provided by Technical Machine, Inc.

Reuse under your choice of:

* BSD-2-Clause
* Apache 2.0
```