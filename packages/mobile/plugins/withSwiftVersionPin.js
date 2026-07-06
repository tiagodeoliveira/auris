// Pin SWIFT_VERSION = 5.0 across all CocoaPods build configurations.
//
// Workaround for a Swift 6.2.3 compiler crash in the SendNonSendable
// pass while compiling expo-modules-core 55.0.25's
// SharedObject.emit(event:arguments:) under Release optimization.
// Symptom: EAS production iOS build fails inside "Run fastlane" with
// a swift-frontend stack trace; preview builds (Debug) succeed.
//
// Remove this plugin once Expo ships an expo-modules-core release
// that compiles cleanly with Swift 6 Release mode, OR once Apple
// ships a Swift toolchain without the SIL optimizer bug.
const { withDangerousMod } = require("@expo/config-plugins");
const fs = require("fs");
const path = require("path");

const HOOK_MARKER = "# AURIS_SWIFT_VERSION_PIN";

module.exports = function withSwiftVersionPin(config) {
  return withDangerousMod(config, [
    "ios",
    async (cfg) => {
      const podfilePath = path.join(cfg.modRequest.platformProjectRoot, "Podfile");
      let podfile = fs.readFileSync(podfilePath, "utf8");
      if (podfile.includes(HOOK_MARKER)) return cfg;

      const injection = `
  ${HOOK_MARKER}
  installer.pods_project.targets.each do |target|
    target.build_configurations.each do |bc|
      bc.build_settings['SWIFT_VERSION'] = '5.0'
    end
  end
`;
      podfile = podfile.replace(/(post_install do \|installer\|\n)/, `$1${injection}`);
      fs.writeFileSync(podfilePath, podfile);
      return cfg;
    },
  ]);
};
