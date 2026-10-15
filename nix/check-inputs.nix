# Content-address only the inputs a check actually reads. In particular, never
# interpolate sourceRoot into a builder: that would make a README edit change
# the builder of an unrelated test even when its executable and fixtures match.
{ lib, sourceRoot }:
let
  checkedPath =
    relative:
    assert lib.assertMsg (
      relative != "" && !(lib.hasPrefix "/" relative) && !(lib.elem ".." (lib.splitString "/" relative))
    ) "check inputs must be repository-relative paths: ${relative}";
    sourceRoot + "/${relative}";
in
{
  # Keep directory structure so Python imports and fixture-relative paths work.
  files =
    paths:
    lib.fileset.toSource {
      root = sourceRoot;
      fileset = lib.fileset.unions (map checkedPath paths);
    };

  # A single file needs no parent tree in its dependency closure.
  file =
    relative:
    builtins.path {
      path = checkedPath relative;
      name = baseNameOf relative;
    };
}
