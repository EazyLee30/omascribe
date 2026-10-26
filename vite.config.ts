import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";

const fromRoot = (path: string) => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig({
  root: fromRoot("./src/"),
  cacheDir: fromRoot("./node_modules/.vite/"),
  build: {
    outDir: fromRoot("./dist/"),
    emptyOutDir: true,
    rollupOptions: {
      input: {
        main: fromRoot("./src/index.html"),
        transcript: fromRoot("./src/transcript.html"),
        captions: fromRoot("./src/captions.html"),
        companion: fromRoot("./src/companion.html"),
      },
      output: {
        manualChunks: {
          editor: [
            "@tiptap/core",
            "@tiptap/starter-kit",
            "@tiptap/markdown",
            "@tiptap/extension-table",
            "@tiptap/extension-task-list",
            "@tiptap/extension-task-item",
            "@tiptap/extension-placeholder",
          ],
        },
      },
    },
  },
});
