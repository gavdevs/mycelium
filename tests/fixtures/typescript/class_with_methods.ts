export interface Greeter {
  greet(name: string): string;
}

export class FormalGreeter implements Greeter {
  prefix: string = "Hello, ";
  greet(name: string): string {
    return this.prefix + name;
  }
}
