// The depth's attribution (depth.ts), apart from the rest so it can be checked on its own.

/** What the depth's attribution names: the sources with depth in view (`inView`, their codes), or
 * every source while that isn't known (null), and the notices their licences ask for. */
export function attribution<S extends { code: number; notice?: string | null }>(sources: S[], inView: Set<number> | null): { here: S[]; notices: string[] } {
  const here = inView ? sources.filter((s) => inView.has(s.code)) : sources;
  return { here, notices: [...new Set(here.map((s) => s.notice).filter((n): n is string => !!n))] };
}
