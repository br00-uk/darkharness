import { helper } from './helper';

export interface Greeter {
    greet(): string;
}

export function addOne(x: number): number {
    return helper(x) + 1;
}
