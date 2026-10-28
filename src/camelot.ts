/** The wheel of Camelot: the same key, a step round the wheel, or the relative major/minor go together. 0 is the best, 3 clashes. */
export function keyFit(a: string, b: string): number {
  const [x, y] = [a.match(/^(\d+)([AB])$/), b.match(/^(\d+)([AB])$/)];
  if (!x || !y) return 3;
  const [nx, ny, lx, ly] = [Number(x[1]), Number(y[1]), x[2], y[2]];
  const round = Math.min((nx - ny + 12) % 12, (ny - nx + 12) % 12);
  if (round === 0) return lx === ly ? 0 : 1;
  if (round === 1 && lx === ly) return 1;
  if (round === 2 && lx === ly) return 2;
  return 3;
}

/** The keys by the number of the wheel: A is minor (the inner ring), B is major (the outer one). */
export const MINOR = ["A♭m", "E♭m", "B♭m", "Fm", "Cm", "Gm", "Dm", "Am", "Em", "Bm", "F♯m", "D♭m"];
export const MAJOR = ["B", "F♯", "D♭", "A♭", "E♭", "B♭", "F", "C", "G", "D", "A", "E"];
