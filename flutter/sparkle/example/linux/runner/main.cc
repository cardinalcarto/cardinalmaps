#include "my_application.h"

int main(int argc, char** argv) {
  // AMD's GL driver (radeonsi) compiles optimized variants of shaders the first time
  // they're drawn with new state, stalling that frame for tens of milliseconds: a stutter
  // whenever tiles bring a new kind of label. The unoptimized shaders are fast enough for
  // a map. Other drivers ignore this, and an AMD_DEBUG of the user's own wins.
  g_setenv("AMD_DEBUG", "nooptvariant", FALSE);

  g_autoptr(MyApplication) app = my_application_new();
  return g_application_run(G_APPLICATION(app), argc, argv);
}
