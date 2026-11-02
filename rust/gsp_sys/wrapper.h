/* Everything the Rust code may call: the GSP facade plus the constants gspc
 * generates for this scene (component keys, property keys, bind ids, action
 * ids). The generated static inline veneers are not visible to Rust; the
 * logic uses the key- and bind-addressed facade they wrap. */
#include "esp_gsp.h"
#include "app_objects.h"
#include "app_actions.h"
#include "app_binds.h"
