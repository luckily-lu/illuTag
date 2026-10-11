export class QueryGeneration {
  private value = 0;
  next() {
    return ++this.value;
  }
  current(token: number) {
    return token === this.value;
  }
  invalidate() {
    this.value++;
  }
}
