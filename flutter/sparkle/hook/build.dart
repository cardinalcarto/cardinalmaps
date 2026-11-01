// Compiles the shaders listed in sparkle.shaderbundle.json into
// build/shaderbundles/sparkle.shaderbundle, which the package lists as an asset.

import 'package:flutter_gpu_shaders/build.dart';
import 'package:hooks/hooks.dart';

void main(List<String> args) async {
  await build(args, (input, output) async {
    await buildShaderBundleJson(
      buildInput: input,
      buildOutput: output,
      manifestFileName: 'sparkle.shaderbundle.json',
      includeDirectories: [input.packageRoot.resolve('shaders/')],
      // GLSL ES 3.00, for `textureLod` in fragment shaders and the like.
      glesLanguageVersion: 300,
    );
  });
}
