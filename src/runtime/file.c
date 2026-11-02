/* File helpers. Appended to io.c by runtime::c_source(), so this file shares its
 * includes, CHEEKU_* status codes, valid_utf8 and Windows headers. Like the input
 * helpers, both exports initialize their out parameter on every path, return a
 * status instead of exiting, and allocate only through malloc/realloc/free so
 * the allocation-accounting tests observe them.
 *
 * Paths are UTF-8 on every host. Windows converts them to UTF-16 for _wfopen so
 * names are independent of the ANSI code page; Linux and macOS pass the bytes to
 * fopen unchanged. Files are always opened in binary mode: no newline
 * translation, and a UTF-8 byte-order mark is ordinary content. */

#define CHEEKU_FILE_INITIAL_CAPACITY 4096

/* Opens path for reading ("rb") or truncating/creating ("wb"). On failure returns
 * NULL and stores CHEEKU_IO, or CHEEKU_ALLOCATION when the Windows UTF-16 path
 * buffer cannot be allocated. A missing file, permission error, directory,
 * empty or over-long path, and an invalid Windows name are all CHEEKU_IO. */
static FILE *open_file(const char *path, int writing, int32_t *status) {
    if (!path || !*path) { *status = CHEEKU_IO; return NULL; }
#ifdef _WIN32
    int count = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, path, -1, NULL, 0);
    if (count <= 0) { *status = CHEEKU_IO; return NULL; }
    WCHAR *wide = (WCHAR *)malloc((size_t)count * sizeof(WCHAR));
    if (!wide) { *status = CHEEKU_ALLOCATION; return NULL; }
    if (MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, path, -1, wide, count) != count) {
        free(wide);
        *status = CHEEKU_IO;
        return NULL;
    }
    FILE *file = _wfopen(wide, writing ? L"wb" : L"rb");
    free(wide);
#else
    FILE *file = fopen(path, writing ? "wb" : "rb");
#endif
    if (!file) *status = CHEEKU_IO;
    return file;
}

/* Reads the whole file into a new NUL-terminated buffer owned by the caller.
 * The reported file size is never trusted (/proc files report zero), so the
 * buffer doubles until end of file. The first NUL byte stops the read, which
 * also bounds endless sources such as /dev/zero. NUL is reported before invalid
 * UTF-8, matching the input helpers. */
int32_t cheeku_read_file(const char *path, char **out) {
    *out = NULL;
    int32_t status = CHEEKU_OK;
    FILE *file = open_file(path, 0, &status);
    if (!file) return status;
    char *buffer = NULL;
    size_t size = 0, capacity = 0;
    for (;;) {
        /* Keep room for at least one byte plus the terminator. */
        if (capacity - size < 2) {
            if (capacity > SIZE_MAX / 2) { status = CHEEKU_OUT_OF_RANGE; break; }
            size_t next = capacity ? capacity * 2 : CHEEKU_FILE_INITIAL_CAPACITY;
            char *grown = (char *)realloc(buffer, next);
            if (!grown) { status = CHEEKU_ALLOCATION; break; }
            buffer = grown;
            capacity = next;
        }
        size_t wanted = capacity - size - 1;
        size_t got = fread(buffer + size, 1, wanted, file);
        if (memchr(buffer + size, 0, got)) { status = CHEEKU_NUL; break; }
        size += got;
        if (got < wanted) {
            if (ferror(file)) status = CHEEKU_IO;
            break;
        }
    }
    fclose(file);
    if (status == CHEEKU_OK && !valid_utf8((const unsigned char *)buffer, size)) {
        status = CHEEKU_INVALID_UTF8;
    }
    if (status != CHEEKU_OK) { free(buffer); return status; }
    buffer[size] = 0;
    /* Return unused capacity so a small file does not pin a multi-kilobyte block
     * for the life of the string. Keeping the larger block is correct if this fails. */
    if (capacity > size + 1) {
        char *fitted = (char *)realloc(buffer, size + 1);
        if (fitted) buffer = fitted;
    }
    *out = buffer;
    return CHEEKU_OK;
}

/* Creates or truncates path and writes content. On success *out is the number of
 * bytes written. content is a Chuchu string, already valid UTF-8 without NUL, so
 * it is written as given with no byte-order mark. Write, flush and close errors
 * (for example a full disk) are all reported. The write is not atomic: a failure
 * after the file was opened can leave it truncated or partially written. */
int32_t cheeku_write_file(const char *path, const char *content, int64_t *out) {
    *out = 0;
    if (!content) return CHEEKU_IO;
    size_t length = strlen(content);
    /* Reject before opening, so an unrepresentable length never truncates a file. */
    if (length > (size_t)INT64_MAX) return CHEEKU_OUT_OF_RANGE;
    int32_t status = CHEEKU_OK;
    FILE *file = open_file(path, 1, &status);
    if (!file) return status;
    if (length && fwrite(content, 1, length, file) != length) status = CHEEKU_IO;
    if (fflush(file) != 0) status = CHEEKU_IO;
    if (fclose(file) != 0) status = CHEEKU_IO;
    if (status != CHEEKU_OK) return status;
    *out = (int64_t)length;
    return CHEEKU_OK;
}
