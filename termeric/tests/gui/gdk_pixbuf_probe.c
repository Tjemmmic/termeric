/* Optional Linux image-decoder diagnostic; not terminal acceptance evidence. */
#include <gdk-pixbuf/gdk-pixbuf.h>
#include <stdio.h>

static unsigned prepared = 0;

static void on_prepared(GdkPixbufLoader *loader, gpointer unused) {
    (void)loader;
    (void)unused;
    prepared++;
}

static void describe(const char *stage, gboolean ok, GdkPixbuf *image, GError **error) {
    printf("%s: ok=%d image=%s prepared=%u", stage, ok, image ? "yes" : "no", prepared);
    if (image) printf(" size=%dx%d", gdk_pixbuf_get_width(image), gdk_pixbuf_get_height(image));
    if (*error) {
        gchar *message = g_strescape((*error)->message, NULL);
        printf(" error=%s", message);
        g_free(message);
    }
    putchar('\n');
    g_clear_error(error);
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fputs("usage: gdk-pixbuf-probe IMAGE\n", stderr);
        return 2;
    }
    GError *error = NULL;
    GSList *formats = gdk_pixbuf_get_formats();
    printf("formats=%u\n", g_slist_length(formats));
    g_slist_free(formats);

    GdkPixbuf *image = gdk_pixbuf_new_from_file(argv[1], &error);
    gboolean file_ok = image != NULL;
    describe("file", file_ok, image, &error);
    if (image) g_object_unref(image);

    gchar *data = NULL;
    gsize length = 0;
    if (!g_file_get_contents(argv[1], &data, &length, &error)) {
        describe("read", FALSE, NULL, &error);
        return 1;
    }
    GdkPixbufLoader *loader = gdk_pixbuf_loader_new();
    g_signal_connect(loader, "area-prepared", G_CALLBACK(on_prepared), NULL);
    gboolean write_ok = gdk_pixbuf_loader_write(loader, (guchar *)data, length, &error);
    describe("write", write_ok, gdk_pixbuf_loader_get_pixbuf(loader), &error);
    gboolean close_ok = gdk_pixbuf_loader_close(loader, &error);
    image = gdk_pixbuf_loader_get_pixbuf(loader);
    gboolean stream_ok = close_ok && image != NULL;
    describe("close", close_ok, image, &error);
    g_object_unref(loader);
    g_free(data);
    return file_ok && write_ok && stream_ok ? 0 : 1;
}
