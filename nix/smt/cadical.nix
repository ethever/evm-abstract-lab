{ pkgs }:
# Both SMT libraries embed a static CaDiCaL copy. Compile that implementation
# privately and carry the same visibility into headers that emit inline methods.
(pkgs.cadical.override { version = "2.1.3"; }).overrideAttrs (old: {
  env = (old.env or { }) // {
    CXXFLAGS = (old.env.CXXFLAGS or "") + " -fvisibility=hidden -fvisibility-inlines-hidden";
  };
  postPatch = (old.postPatch or "") + ''
    substituteInPlace src/cadical.hpp src/tracer.hpp \
      --replace-fail 'namespace CaDiCaL {' \
        'namespace __attribute__((visibility("hidden"))) CaDiCaL {'
  '';
})
