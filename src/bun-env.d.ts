// Ambient types for Bun's bundler (replaces vite/client). Asset imports resolve
// to a URL string; CSS imports have no runtime value. `import.meta.hot` typing
// comes from the "bun" types referenced in tsconfig.
declare module "*.svg" {
  const url: string;
  export default url;
}
declare module "*.png" {
  const url: string;
  export default url;
}
declare module "*.css" {}
