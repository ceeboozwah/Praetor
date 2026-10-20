// bench/jsonparse — DOM-parse argv[1] (same JSON subset: objects,
// arrays, no-escape strings, ints, bool, null), count nodes, free.
// Track A port of jsonparse.c: same parser, same node layout.
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

enum Tag { NUM, STR, ARR, OBJ, BOOL, NUL };
struct Node {
    Tag tag;
    int64_t num;
    Node **kids;    /* ARR / OBJ */
    uint64_t nkids;
    char *str;      /* STR */
    char **keys;    /* OBJ */
};

struct Cur { const char *b; uint64_t i, n; };

static void ws(Cur *p) {
    while (p->i < p->n && (p->b[p->i] == ' ' || p->b[p->i] == '\n' ||
                           p->b[p->i] == '\t' || p->b[p->i] == '\r'))
        p->i++;
}

static Node *jparse(Cur *p);

static int lit(Cur *p, const char *s) {
    uint64_t n = strlen(s);
    if (p->i + n <= p->n && memcmp(p->b + p->i, s, n) == 0) {
        p->i += n;
        return 1;
    }
    return 0;
}

static Node *mk(Tag t) {
    Node *x = (Node *)calloc(1, sizeof(Node));
    x->tag = t;
    return x;
}

static Node *jstr(Cur *p) {
    ws(p);
    if (p->i >= p->n || p->b[p->i] != '"') return NULL;
    p->i++;
    uint64_t lo = p->i;
    while (p->i < p->n && p->b[p->i] != '"') p->i++;
    if (p->i >= p->n) return NULL;
    Node *x = mk(STR);
    x->str = (char *)malloc(p->i - lo + 1);
    memcpy(x->str, p->b + lo, p->i - lo);
    x->str[p->i - lo] = 0;
    p->i++;
    return x;
}

static Node *jnum(Cur *p) {
    int64_t v = 0, neg = 0;
    if (p->i < p->n && p->b[p->i] == '-') { neg = 1; p->i++; }
    if (p->i >= p->n || p->b[p->i] < '0' || p->b[p->i] > '9') return NULL;
    while (p->i < p->n && p->b[p->i] >= '0' && p->b[p->i] <= '9')
        v = v * 10 + (p->b[p->i++] - '0');
    Node *x = mk(NUM);
    x->num = neg ? -v : v;
    return x;
}

static void push(Node *a, Node *k) {
    a->kids = (Node **)realloc(a->kids, (a->nkids + 1) * sizeof(Node *));
    a->kids[a->nkids++] = k;
}

static Node *jarr(Cur *p) {
    p->i++;
    Node *x = mk(ARR);
    ws(p);
    if (p->i < p->n && p->b[p->i] == ']') { p->i++; return x; }
    for (;;) {
        Node *v = jparse(p);
        if (!v) return NULL;
        push(x, v);
        ws(p);
        if (p->i >= p->n) return NULL;
        if (p->b[p->i] == ',') { p->i++; continue; }
        if (p->b[p->i] == ']') { p->i++; return x; }
        return NULL;
    }
}

static Node *jobj(Cur *p) {
    p->i++;
    Node *x = mk(OBJ);
    ws(p);
    if (p->i < p->n && p->b[p->i] == '}') { p->i++; return x; }
    for (;;) {
        Node *k = jstr(p);
        if (!k) return NULL;
        ws(p);
        if (p->i >= p->n || p->b[p->i] != ':') return NULL;
        p->i++;
        Node *v = jparse(p);
        if (!v) return NULL;
        x->keys = (char **)realloc(x->keys,
                                   (x->nkids + 1) * sizeof(char *));
        x->keys[x->nkids] = k->str;
        free(k);
        push(x, v);
        ws(p);
        if (p->i >= p->n) return NULL;
        if (p->b[p->i] == ',') { p->i++; continue; }
        if (p->b[p->i] == '}') { p->i++; return x; }
        return NULL;
    }
}

static Node *jparse(Cur *p) {
    ws(p);
    if (p->i >= p->n) return NULL;
    char c = p->b[p->i];
    if (c == '{') return jobj(p);
    if (c == '[') return jarr(p);
    if (c == '"') return jstr(p);
    if (c == 't' && lit(p, "true")) { Node *x = mk(BOOL); x->num = 1; return x; }
    if (c == 'f' && lit(p, "false")) { Node *x = mk(BOOL); x->num = 0; return x; }
    if (c == 'n' && lit(p, "null")) return mk(NUL);
    if (c == '-' || (c >= '0' && c <= '9')) return jnum(p);
    return NULL;
}

static uint64_t count(const Node *x) {
    uint64_t t = 1;
    for (uint64_t i = 0; i < x->nkids; i++) t += count(x->kids[i]);
    return t;
}

static void jfree(Node *x) {
    for (uint64_t i = 0; i < x->nkids; i++) jfree(x->kids[i]);
    for (uint64_t i = 0; x->keys && i < x->nkids; i++) free(x->keys[i]);
    free(x->keys);
    free(x->kids);
    free(x->str);
    free(x);
}

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    int fd = open(argv[1], O_RDONLY);
    if (fd < 0) return 2;
    struct stat st;
    if (fstat(fd, &st)) return 2;
    char *buf = (char *)malloc((size_t)st.st_size + 1);
    if (read(fd, buf, (size_t)st.st_size) != st.st_size) return 2;
    close(fd);
    buf[st.st_size] = 0;
    Cur p = {buf, 0, (uint64_t)st.st_size};
    Node *r = jparse(&p);
    if (!r) return 3;
    printf("%llu\n", (unsigned long long)count(r));
    jfree(r);
    free(buf);
    return 0;
}
