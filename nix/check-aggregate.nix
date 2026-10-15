# A transparent group only links already-public checks; it performs no extra
# assertions. The development selector may expand this explicit contract into
# changed leaves, avoiding uncached unrelated siblings. Full flake checks still
# build every member. Never attach this metadata to a builder with assertions.
{ pkgs }:
{
  name,
  checks,
  links ? [ ],
}:
(pkgs.linkFarm name (
  pkgs.lib.mapAttrsToList (name: path: { inherit name path; }) checks
  # Compatibility artifact links must come from the declared checks above.
  ++ links
)).overrideAttrs
  (_: {
    passthru.checkAggregateDependencies = builtins.attrValues checks;
  })
