// A Java agent for the Apalache server (tools/quint-jvm/README.md): raises Jackson's default
// StreamReadConstraints before Apalache runs, so the remote configuration, which carries the
// whole Quint IR as one JSON string, may exceed Jackson's 20,000,000-character default.
// Apalache 0.62 parses it with a plain `new JsonFactory()`, and Jackson 2.x reads no system
// property for the limit; only StreamReadConstraints.overrideDefaultStreamReadConstraints.
//
//   java -javaagent:jackson-limits.jar[=MAX_STRING_LEN] -jar apalache.jar server ...
//
// MAX_STRING_LEN defaults to 200,000,000 characters. Only the string-length limit changes.
import com.fasterxml.jackson.core.StreamReadConstraints;

public final class JacksonLimits {
    public static void premain(String args) {
        int max = 200_000_000;
        if (args != null && !args.isEmpty()) max = Integer.parseInt(args.trim());
        StreamReadConstraints.overrideDefaultStreamReadConstraints(
            StreamReadConstraints.defaults().rebuild().maxStringLength(max).build());
        System.err.println("jackson-limits: StreamReadConstraints maxStringLength " + max);
    }
}
