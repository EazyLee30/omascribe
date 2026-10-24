import { defineConfig } from "vite";
export default defineConfig({
  build: {
    rollupOptions: {
      input: { main: "index.html", transcript: "transcript.html", captions: "captions.html" },
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
