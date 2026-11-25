#include <locale.h>
#include <stdio.h>
#include <wchar.h>
#include <stdlib.h>
int main(void) {
    setlocale(LC_ALL, "");
    const char *s = "caf\xc3\xa9";
    mbstate_t st = {0};
    wchar_t c;
    printf("MB_CUR_MAX=%d\n", (int)MB_CUR_MAX);
    while (*s) {
        size_t n = mbrtowc(&c, s, MB_CUR_MAX, &st);
        printf("byte 0x%02x -> %ld (c=0x%lx)\n", (unsigned char)*s, (long)n, (unsigned long)c);
        if (n == (size_t)-1 || n == (size_t)-2) break;
        s += n;
    }
    return 0;
}
