// Sample JavaScript — syntax highlighting demo.
const greet = (name = "world") => `Hello, ${name}!`;

async function fetchJson(url) {
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`HTTP ${res.status}`);
  }
  return res.json();
}

class Counter {
  #count = 0;
  increment(by = 1) {
    this.#count += by;
    return this.#count;
  }
}

const doubled = [1, 2, 3].map((n) => n * 2);
const c = new Counter();
console.log(greet("konoma"), doubled, c.increment(5));
