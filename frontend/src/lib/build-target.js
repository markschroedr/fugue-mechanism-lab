/**
 * Resolve a requested SvelteKit build target against a registry of adapter
 * factories.
 *
 * The selector fails closed: an unknown target name throws with the list of
 * known targets instead of silently defaulting. An unset (`undefined`) request
 * means ordinary local/dev work and resolves to `defaultTarget`.
 *
 * The registry is injected rather than imported here so this logic can be
 * tested without pulling in Node-only adapter packages.
 *
 * @template A
 * @param {string | undefined} requested
 * @param {Record<string, () => A>} targets
 * @param {string} [defaultTarget]
 * @returns {{ name: string; adapter: A }}
 */
export function resolveBuildTarget(requested, targets, defaultTarget = "hosted") {
  const name = requested ?? defaultTarget;
  const factory = targets[name];
  if (!factory) {
    const known = Object.keys(targets).join(", ");
    throw new Error(`Unknown FUGUE_BUILD_TARGET "${name}". Expected one of: ${known}.`);
  }
  return { name, adapter: factory() };
}
