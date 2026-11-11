const strings = require("../util/strings");
const { shout } = require("../util/strings");

class Counter {
  constructor() {
    this.count = 0;
  }

  increment() {
    this.count += 1;
    return this;
  }
}

function run() {
  const c = new Counter();
  c.increment().increment();
  return strings.countWords(shout("a b")) + c.count;
}

module.exports = { Counter, run };
