// Exercise the installed DSOs in both RTLD_GLOBAL load orders. Their CaDiCaL
// implementation must stay private even when the consumer exports everything.
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
  if (argc != 5) return EXIT_FAILURE;
  void *handles[2];
  for (int i = 0; i < 2; ++i) {
    handles[i] = dlopen(argv[1 + i * 2], RTLD_NOW | RTLD_GLOBAL);
    if (!handles[i]) {
      fprintf(stderr, "%s\n", dlerror());
      return EXIT_FAILURE;
    }
  }
  for (int i = 0; i < 2; ++i) {
    if (!dlsym(handles[i], argv[2 + i * 2])) {
      fprintf(stderr, "public solver API unavailable: %s\n", dlerror());
      return EXIT_FAILURE;
    }
    if (dlsym(handles[i], "_ZN7CaDiCaL6Solver5solveEv")) {
      fprintf(stderr, "private CaDiCaL implementation escaped: %s\n", argv[1 + i * 2]);
      return EXIT_FAILURE;
    }
  }
  for (int i = 1; i >= 0; --i) dlclose(handles[i]);
  return EXIT_SUCCESS;
}
