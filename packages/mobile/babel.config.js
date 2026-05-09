// SDK 50+: `babel-preset-expo` includes the expo-router plugin
// transparently, so no extra plugins entry is needed here.
module.exports = function (api) {
  api.cache(true);
  return {
    presets: ["babel-preset-expo"],
  };
};
