#ifndef GRAPHA_INDEXSTORE_H
#define GRAPHA_INDEXSTORE_H
#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

typedef struct { const char *data; size_t length; } indexstore_string_ref_t;

// Opaque types — use struct pointers for type safety
typedef struct indexstore_s *indexstore_t;
typedef struct indexstore_unit_reader_s *indexstore_unit_reader_t;
typedef struct indexstore_record_reader_s *indexstore_record_reader_t;
typedef struct indexstore_symbol_s *indexstore_symbol_t;
typedef struct indexstore_occurrence_s *indexstore_occurrence_t;
typedef struct indexstore_symbol_relation_s *indexstore_symbol_relation_t;
typedef struct indexstore_unit_dependency_s *indexstore_unit_dependency_t;
typedef struct indexstore_error_s *indexstore_error_t;

indexstore_t indexstore_store_create(const char *path, indexstore_error_t *error);
void indexstore_store_dispose(indexstore_t);

bool indexstore_store_units_apply_f(indexstore_t store, int reserved, void *ctx,
    bool (*callback)(void *ctx, const char *name, size_t name_len));

indexstore_unit_reader_t indexstore_unit_reader_create(indexstore_t, const char *name, indexstore_error_t *error);
void indexstore_unit_reader_dispose(indexstore_unit_reader_t);
indexstore_string_ref_t indexstore_unit_reader_get_main_file(indexstore_unit_reader_t);
indexstore_string_ref_t indexstore_unit_reader_get_module_name(indexstore_unit_reader_t);

bool indexstore_unit_reader_dependencies_apply_f(indexstore_unit_reader_t, void *ctx,
    bool (*callback)(void *ctx, indexstore_unit_dependency_t));
int indexstore_unit_dependency_get_kind(indexstore_unit_dependency_t);
indexstore_string_ref_t indexstore_unit_dependency_get_name(indexstore_unit_dependency_t);

indexstore_record_reader_t indexstore_record_reader_create(indexstore_t, const char *name, indexstore_error_t *error);
void indexstore_record_reader_dispose(indexstore_record_reader_t);

bool indexstore_record_reader_occurrences_apply_f(indexstore_record_reader_t, void *ctx,
    bool (*callback)(void *ctx, indexstore_occurrence_t));

indexstore_symbol_t indexstore_occurrence_get_symbol(indexstore_occurrence_t);
uint64_t indexstore_occurrence_get_roles(indexstore_occurrence_t);
void indexstore_occurrence_get_line_col(indexstore_occurrence_t, unsigned *line, unsigned *col);

bool indexstore_occurrence_relations_apply_f(indexstore_occurrence_t, void *ctx,
    bool (*callback)(void *ctx, indexstore_symbol_relation_t));

indexstore_string_ref_t indexstore_symbol_get_name(indexstore_symbol_t);
indexstore_string_ref_t indexstore_symbol_get_usr(indexstore_symbol_t);
uint64_t indexstore_symbol_get_kind(indexstore_symbol_t);
uint64_t indexstore_symbol_get_roles(indexstore_symbol_t);

indexstore_symbol_t indexstore_symbol_relation_get_symbol(indexstore_symbol_relation_t);
uint64_t indexstore_symbol_relation_get_roles(indexstore_symbol_relation_t);

const char *indexstore_error_get_description(indexstore_error_t);
void indexstore_error_dispose(indexstore_error_t);
#endif
