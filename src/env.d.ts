/// <reference types="vite/client" />
declare module "*.vue" {
  import type { DefineComponent } from "vue";
  const component: DefineComponent<object, object, unknown>;
  export default component;
}

/** The version of the app (from package.json). */
declare const __APP_VERSION__: string;
