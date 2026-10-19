import { createApp } from "vue";
import { createVuetify } from "vuetify";
import * as components from "vuetify/components";
import * as directives from "vuetify/directives";
import { aliases, mdi } from "vuetify/iconsets/mdi-svg";
import "vuetify/styles";
import App from "./App.vue";
import "./styles.css";

const vuetify = createVuetify({
  components,
  directives,
  theme: {
    // The system's appearance, like any Mac app: the LCD stays dark in both.
    defaultTheme: "system",
    themes: {
      light: { dark: false, colors: { background: "#f5f5f7", surface: "#f5f5f7", primary: "#007aff", secondary: "#6e6e73" } },
      dark: { dark: true, colors: { background: "#1c1c1e", surface: "#1c1c1e", primary: "#0a84ff", secondary: "#98989d" } },
    },
  },
  icons: { defaultSet: "mdi", aliases, sets: { mdi } },
  defaults: { VBtn: { rounded: "pill" } },
});

// A window that is not the key one shows its selection in gray, as every Mac app does.
const markActive = () => document.documentElement.classList.toggle("inactive", !document.hasFocus());
window.addEventListener("focus", markActive);
window.addEventListener("blur", markActive);
markActive();

document.documentElement.classList.toggle("mac", /Mac/.test(navigator.userAgent));

// No browser menu (Reload, Inspect Element): the app is not a web page. The rows of the queue have a menu of their own.
window.addEventListener("contextmenu", (event) => event.preventDefault());

createApp(App).use(vuetify).mount("#app");
