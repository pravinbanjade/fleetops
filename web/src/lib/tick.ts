/**
 * Which characters of `next` differ from `prev`, the two aligned on their
 * right edge: figures keep their units on the right, so 9.8% → 10.2% lines
 * the "%" and the tenths up and marks the new tens digit.
 */
export function diffDigits(prev: string, next: string): boolean[] {
  const shift = next.length - prev.length;
  return [...next].map((c, i) => {
    const j = i - shift;
    return j < 0 || prev[j] !== c;
  });
}
