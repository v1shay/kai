#include <ghostty/vt.h>
#include <stdlib.h>

typedef void (*Bytes)(void *, const uint8_t *, size_t);
typedef void (*Cell)(void *, size_t, size_t, size_t, const uint8_t *, size_t, uint32_t, uint32_t, uint8_t);

typedef struct {
    GhosttyTerminal terminal;
    GhosttyRenderState render;
    GhosttyRenderStateRowIterator rows;
    GhosttyRenderStateRowCells cells;
    void *context;
    Bytes reply, title;
    GhosttySizeReportSize size;
} Engine;

typedef struct {
    size_t display_offset;
    uint16_t cursor_x, cursor_y;
    bool cursor, app_cursor, bracketed_paste, alternate;
} Viewport;

static const GhosttyColorRgb foreground = { 213, 217, 220 };
static const GhosttyColorRgb background = { 32, 35, 38 };

static void reply(GhosttyTerminal terminal, void *userdata, const uint8_t *data, size_t len) {
    Engine *engine = userdata;
    engine->reply(engine->context, data, len);
}

static void title_changed(GhosttyTerminal terminal, void *userdata) {
    Engine *engine = userdata;
    GhosttyString title = {0};
    if (ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_TITLE, &title) == GHOSTTY_SUCCESS)
        engine->title(engine->context, title.ptr, title.len);
}

static bool report_size(GhosttyTerminal terminal, void *userdata, GhosttySizeReportSize *out) {
    *out = ((Engine *)userdata)->size;
    return true;
}

static void destroy(Engine *engine) {
    if (!engine) return;
    ghostty_render_state_row_cells_free(engine->cells);
    ghostty_render_state_row_iterator_free(engine->rows);
    ghostty_render_state_free(engine->render);
    ghostty_terminal_free(engine->terminal);
    free(engine);
}

static bool resize(Engine *engine, uint16_t cols, uint16_t rows, uint16_t width, uint16_t height) {
    engine->size = (GhosttySizeReportSize){ .columns = cols, .rows = rows,
        .cell_width = width, .cell_height = height };
    return ghostty_terminal_resize(engine->terminal, cols, rows, width, height) == GHOSTTY_SUCCESS;
}

static Engine *create(uint16_t cols, uint16_t rows, uint16_t width, uint16_t height,
                      size_t history, void *context, Bytes send_reply, Bytes set_title) {
    Engine *engine = calloc(1, sizeof(*engine));
    if (!engine) return NULL;
    engine->context = context;
    engine->reply = send_reply;
    engine->title = set_title;
    if (ghostty_terminal_new(NULL, &engine->terminal, cols, rows) ||
        ghostty_render_state_new(NULL, &engine->render) ||
        ghostty_render_state_row_iterator_new(NULL, &engine->rows) ||
        ghostty_render_state_row_cells_new(NULL, &engine->cells) ||
        ghostty_terminal_set(engine->terminal, GHOSTTY_TERMINAL_OPT_USERDATA, engine) ||
        ghostty_terminal_set(engine->terminal, GHOSTTY_TERMINAL_OPT_WRITE_PTY, reply) ||
        ghostty_terminal_set(engine->terminal, GHOSTTY_TERMINAL_OPT_TITLE_CHANGED, title_changed) ||
        ghostty_terminal_set(engine->terminal, GHOSTTY_TERMINAL_OPT_SIZE, report_size) ||
        ghostty_terminal_set(engine->terminal, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES, &history) ||
        ghostty_terminal_set(engine->terminal, GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, &foreground) ||
        ghostty_terminal_set(engine->terminal, GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, &background) ||
        !resize(engine, cols, rows, width, height)) {
        destroy(engine);
        return NULL;
    }
    // Clipboard, window resize and notification requests have no host callbacks.
    return engine;
}

static void write_vt(Engine *engine, const uint8_t *data, size_t len) {
    ghostty_terminal_vt_write(engine->terminal, data, len);
}

static void scroll(Engine *engine, int32_t lines) {
    GhosttyTerminalScrollViewport value = {
        .tag = lines ? GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA : GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM,
        .value = { .delta = -(int64_t)lines }
    };
    ghostty_terminal_scroll_viewport(engine->terminal, value);
    if (!lines) ghostty_terminal_set(engine->terminal, GHOSTTY_TERMINAL_OPT_SELECTION, NULL);
}

static void select_cells(Engine *engine, uint16_t row, uint16_t col, bool start) {
    GhosttyPoint point = { .tag = GHOSTTY_POINT_TAG_VIEWPORT, .value = { .coordinate = { .x = col, .y = row } } };
    GhosttySelection range = {0};
    if (!start && ghostty_terminal_get(engine->terminal, GHOSTTY_TERMINAL_DATA_SELECTION, &range)) return;
    if (ghostty_terminal_grid_ref(engine->terminal, point, &range.end)) return;
    if (start) range.start = range.end;
    ghostty_terminal_set(engine->terminal, GHOSTTY_TERMINAL_OPT_SELECTION, &range);
}

static void select_all(Engine *engine) {
    GhosttySelection range = {0};
    if (!ghostty_terminal_select_all(engine->terminal, &range))
        ghostty_terminal_set(engine->terminal, GHOSTTY_TERMINAL_OPT_SELECTION, &range);
}

static size_t copy_selection(Engine *engine, uint8_t *buffer, size_t capacity) {
    GhosttySelection range = {0};
    GhosttyFormatter formatter = NULL;
    if (ghostty_terminal_get(engine->terminal, GHOSTTY_TERMINAL_DATA_SELECTION, &range) ||
        ghostty_formatter_terminal_new(NULL, &formatter, engine->terminal)) return 0;
    bool trim = true;
    size_t written = 0;
    if (ghostty_formatter_set(formatter, GHOSTTY_FORMATTER_OPT_SELECTION, &range) ||
        ghostty_formatter_set(formatter, GHOSTTY_FORMATTER_OPT_TRIM, &trim) ||
        ghostty_formatter_format_buf(formatter, buffer, capacity, &written)) written = 0;
    ghostty_formatter_free(formatter);
    return written;
}

static uint32_t rgb(GhosttyColorRgb color) {
    return (uint32_t)color.r << 16 | (uint32_t)color.g << 8 | color.b;
}

static bool mode(Engine *engine, GhosttyMode value) {
    GhosttyTerminalModeConfig config = { .mode = value };
    return !ghostty_terminal_get(engine->terminal, GHOSTTY_TERMINAL_DATA_MODE, &config) && config.value;
}

static void snapshot(Engine *engine, Viewport *viewport, void *output, Cell emit) {
    if (ghostty_render_state_update(engine->render, engine->terminal) ||
        ghostty_render_state_get(engine->render, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, &engine->rows)) return;

    GhosttyRenderStateCursor cursor = GHOSTTY_INIT_SIZED(GhosttyRenderStateCursor);
    if (!ghostty_render_state_get(engine->render, GHOSTTY_RENDER_STATE_DATA_CURSOR, &cursor)) {
        viewport->cursor = cursor.visible && cursor.viewport_has_value;
        viewport->cursor_x = cursor.viewport_x;
        viewport->cursor_y = cursor.viewport_y;
    }
    viewport->app_cursor = mode(engine, GHOSTTY_MODE_CURSOR_KEYS);
    viewport->bracketed_paste = mode(engine, GHOSTTY_MODE_BRACKETED_PASTE);
    GhosttyTerminalScreen screen;
    if (!ghostty_terminal_get(engine->terminal, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &screen))
        viewport->alternate = screen == GHOSTTY_TERMINAL_SCREEN_ALTERNATE;
    GhosttyTerminalScrollbar scrollbar;
    if (!ghostty_terminal_get(engine->terminal, GHOSTTY_TERMINAL_DATA_SCROLLBAR, &scrollbar) &&
        scrollbar.total > scrollbar.offset + scrollbar.len)
        viewport->display_offset = scrollbar.total - scrollbar.offset - scrollbar.len;

    for (size_t y = 0; ghostty_render_state_row_iterator_next(engine->rows); y++) {
        if (ghostty_render_state_row_get(engine->rows, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, &engine->cells)) continue;
        for (size_t x = 0; ghostty_render_state_row_cells_next(engine->cells); x++) {
            GhosttyCell cell;
            GhosttyCellWide width;
            if (ghostty_render_state_row_cells_get(engine->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW, &cell) ||
                ghostty_cell_get(cell, GHOSTTY_CELL_DATA_WIDE, &width) || width >= GHOSTTY_CELL_WIDE_SPACER_TAIL) continue;
            uint8_t text[128];
            GhosttyBuffer buffer = { .ptr = text, .cap = sizeof(text) };
            if (ghostty_render_state_row_cells_get(engine->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8, &buffer) || !buffer.len) {
                text[0] = ' ';
                buffer.len = 1;
            }
            uint32_t fg = rgb(foreground), bg = UINT32_MAX;
            uint8_t flags = 0;
            bool styled = false, selected = false;
            ghostty_render_state_row_cells_get(engine->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING, &styled);
            if (styled) {
                GhosttyStyle style;
                if (!ghostty_render_state_row_cells_get(engine->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, &style)) {
                    GhosttyColorRgb color;
                    if (!ghostty_render_state_row_cells_get(engine->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR, &color)) fg = rgb(color);
                    if (!ghostty_render_state_row_cells_get(engine->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR, &color)) bg = rgb(color);
                    flags = (style.flags & 3) | ((GHOSTTY_STYLE_UNDERLINE(style) != 0) << 2) |
                        ((bool)(style.flags & GHOSTTY_STYLE_FLAG_STRIKETHROUGH) << 3);
                    if (style.flags & GHOSTTY_STYLE_FLAG_INVERSE) {
                        uint32_t old = fg;
                        fg = bg == UINT32_MAX ? rgb(background) : bg;
                        bg = old;
                    }
                    if (style.flags & GHOSTTY_STYLE_FLAG_INVISIBLE) { text[0] = ' '; buffer.len = 1; }
                }
            }
            if (!ghostty_render_state_row_cells_get(engine->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_SELECTED, &selected) && selected)
                bg = 0x365a78;
            emit(output, y, x, width == GHOSTTY_CELL_WIDE_WIDE ? 2 : 1, text, buffer.len, fg, bg, flags);
        }
    }
}

typedef struct {
    Engine *(*create)(uint16_t, uint16_t, uint16_t, uint16_t, size_t, void *, Bytes, Bytes);
    void (*destroy)(Engine *);
    bool (*resize)(Engine *, uint16_t, uint16_t, uint16_t, uint16_t);
    void (*write)(Engine *, const uint8_t *, size_t);
    void (*scroll)(Engine *, int32_t);
    void (*select_cells)(Engine *, uint16_t, uint16_t, bool);
    void (*select_all)(Engine *);
    size_t (*copy)(Engine *, uint8_t *, size_t);
    void (*snapshot)(Engine *, Viewport *, void *, Cell);
} Bridge;

__attribute__((visibility("default"))) const Bridge *kai_terminal_ghostty_v1(void) {
    static const Bridge bridge = { create, destroy, resize, write_vt, scroll, select_cells, select_all, copy_selection, snapshot };
    return &bridge;
}
