// Babel config for the Expo mobile app.
//
// `babel-preset-expo` covers expo-router transparently (SDK 50+).
// `react-native-reanimated/plugin` MUST be the LAST plugin in the
// list — Reanimated's worklet transform relies on running after
// every other transform has had a chance to rewrite the source.
module.exports = function (api) {
  api.cache(true);
  return {
    presets: ["babel-preset-expo"],
    plugins: ["react-native-reanimated/plugin"],
  };
};
