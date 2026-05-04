import { add } from "./simple_function";
import type { Greeter } from "./class_with_methods";

export const greet: Greeter = {
  greet(name: string): string {
    return `Hi ${name}, sum is ${add(1, 2)}`;
  },
};
