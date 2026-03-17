#include <stdio.h>
#include <stdlib.h>
#include <time.h>

static const uint32_t PALETTE[64] = {
    0x000000, 0x222222, 0x444444, 0x666666, 0x999999, 0xbbbbbb, 0xdddddd, 0xffffff,
    0x220000, 0x440000, 0x660000, 0x780000, 0x9a0000, 0xbc0000, 0xde0000, 0xff0000,
    0x221100, 0x442200, 0x663300, 0x783c00, 0x9a4d00, 0xbc5e00, 0xde6f00, 0xff7f00,
    0x222200, 0x444400, 0x666600, 0x787800, 0x999a00, 0xbbbc00, 0xddde00, 0xffff00,
    0x002200, 0x004400, 0x006600, 0x007800, 0x009a00, 0x00bc00, 0x00de00, 0x00ff00,
    0x002222, 0x004444, 0x006666, 0x007878, 0x00999a, 0x00bbbc, 0x00ddde, 0x00feff,
    0x000022, 0x000044, 0x000066, 0x000078, 0x00009a, 0x0000bc, 0x0000de, 0x0000ff,
    0x220022, 0x440044, 0x660066, 0x780078, 0x9a0099, 0xbc00bb, 0xde00dd, 0xff00fe,
};

static double now_secs(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec * 1e-9;
}

static void render_framebuffer(const uint8_t fb[256], long tick_count, double tps) {
    printf("\033[H");
    for (int y = 0; y < 16; y++) {
        for (int x = 0; x < 16; x++) {
            uint32_t c = PALETTE[fb[y * 16 + x] % 64];
            uint8_t r = (c >> 16) & 0xff;
            uint8_t g = (c >> 8) & 0xff;
            uint8_t b = c & 0xff;
            printf("\033[48;2;%d;%d;%dm  ", r, g, b);
        }
        printf("\033[0m\n");
    }
    printf("tick %ld  %.0f ticks/sec\033[K\n", tick_count, tps);
}

int main(void) {
    SpreadsheetState *state = spreadsheet_init();

    for (int i = 0; i < 10; i++)
        cpu_tick(state, 0.0, true);

    printf("\033[2J");
    printf("\033[?25l");

    double render_interval = 1.0 / 60.0;
    double last_render = now_secs();
    double start = last_render;
    long tick_count = 0;
    CpuTickOutput out;

    for (;;) {
        out = cpu_tick(state, 0.0, false);
        tick_count++;

        double now = now_secs();
        if (now - last_render >= render_interval) {
            double tps = tick_count / (now - start);
            render_framebuffer(out.framebuffer, tick_count, tps);
            last_render = now;
        }
    }

    printf("\033[?25h");
    spreadsheet_free(state);
    return 0;
}
