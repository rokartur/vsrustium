pub struct Template {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub content: &'static str,
}

/// taskTemplates.ts `getTemplates`: in the order and sort upstream returns them.
pub fn templates() -> Vec<Template> {
    vec![MSBUILD, MAVEN, DOTNET_BUILD, COMMAND]
}

const DOTNET_BUILD: Template = Template {
    id: "dotnetCore",
    label: ".NET Core",
    description: "Executes .NET Core build command",
    content: concat!(
        "{\n",
        "\t// See https://go.microsoft.com/fwlink/?LinkId=733558\n",
        "\t// for the documentation about the tasks.json format\n",
        "\t\"version\": \"2.0.0\",\n",
        "\t\"tasks\": [\n",
        "\t\t{\n",
        "\t\t\t\"label\": \"build\",\n",
        "\t\t\t\"command\": \"dotnet\",\n",
        "\t\t\t\"type\": \"shell\",\n",
        "\t\t\t\"args\": [\n",
        "\t\t\t\t\"build\",\n",
        "\t\t\t\t// Ask dotnet build to generate full paths for file names.\n",
        "\t\t\t\t\"/property:GenerateFullPaths=true\",\n",
        "\t\t\t\t// Do not generate summary otherwise it leads to duplicate errors in Problems panel\n",
        "\t\t\t\t\"/consoleloggerparameters:NoSummary\"\n",
        "\t\t\t],\n",
        "\t\t\t\"group\": \"build\",\n",
        "\t\t\t\"presentation\": {\n",
        "\t\t\t\t\"reveal\": \"silent\"\n",
        "\t\t\t},\n",
        "\t\t\t\"problemMatcher\": \"$msCompile\"\n",
        "\t\t}\n",
        "\t]\n",
        "}",
    ),
};

const MSBUILD: Template = Template {
    id: "msbuild",
    label: "MSBuild",
    description: "Executes the build target",
    content: concat!(
        "{\n",
        "\t// See https://go.microsoft.com/fwlink/?LinkId=733558\n",
        "\t// for the documentation about the tasks.json format\n",
        "\t\"version\": \"2.0.0\",\n",
        "\t\"tasks\": [\n",
        "\t\t{\n",
        "\t\t\t\"label\": \"build\",\n",
        "\t\t\t\"type\": \"shell\",\n",
        "\t\t\t\"command\": \"msbuild\",\n",
        "\t\t\t\"args\": [\n",
        "\t\t\t\t// Ask msbuild to generate full paths for file names.\n",
        "\t\t\t\t\"/property:GenerateFullPaths=true\",\n",
        "\t\t\t\t\"/t:build\",\n",
        "\t\t\t\t// Do not generate summary otherwise it leads to duplicate errors in Problems panel\n",
        "\t\t\t\t\"/consoleloggerparameters:NoSummary\"\n",
        "\t\t\t],\n",
        "\t\t\t\"group\": \"build\",\n",
        "\t\t\t\"presentation\": {\n",
        "\t\t\t\t// Reveal the output only if unrecognized errors occur.\n",
        "\t\t\t\t\"reveal\": \"silent\"\n",
        "\t\t\t},\n",
        "\t\t\t// Use the standard MS compiler pattern to detect errors, warnings and infos\n",
        "\t\t\t\"problemMatcher\": \"$msCompile\"\n",
        "\t\t}\n",
        "\t]\n",
        "}",
    ),
};

const COMMAND: Template = Template {
    id: "externalCommand",
    label: "Others",
    description: "Example to run an arbitrary external command",
    content: concat!(
        "{\n",
        "\t// See https://go.microsoft.com/fwlink/?LinkId=733558\n",
        "\t// for the documentation about the tasks.json format\n",
        "\t\"version\": \"2.0.0\",\n",
        "\t\"tasks\": [\n",
        "\t\t{\n",
        "\t\t\t\"label\": \"echo\",\n",
        "\t\t\t\"type\": \"shell\",\n",
        "\t\t\t\"command\": \"echo Hello\"\n",
        "\t\t}\n",
        "\t]\n",
        "}",
    ),
};

const MAVEN: Template = Template {
    id: "maven",
    label: "maven",
    description: "Executes common maven commands",
    content: concat!(
        "{\n",
        "\t// See https://go.microsoft.com/fwlink/?LinkId=733558\n",
        "\t// for the documentation about the tasks.json format\n",
        "\t\"version\": \"2.0.0\",\n",
        "\t\"tasks\": [\n",
        "\t\t{\n",
        "\t\t\t\"label\": \"verify\",\n",
        "\t\t\t\"type\": \"shell\",\n",
        "\t\t\t\"command\": \"mvn -B verify\",\n",
        "\t\t\t\"group\": \"build\"\n",
        "\t\t},\n",
        "\t\t{\n",
        "\t\t\t\"label\": \"test\",\n",
        "\t\t\t\"type\": \"shell\",\n",
        "\t\t\t\"command\": \"mvn -B test\",\n",
        "\t\t\t\"group\": \"test\"\n",
        "\t\t}\n",
        "\t]\n",
        "}",
    ),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn others_template_parses_after_stripping_comments() {
        let all = templates();
        assert_eq!(all.len(), 4);
        let others = all.iter().find(|t| t.id == "externalCommand").unwrap();
        let json: String = others
            .content
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["tasks"][0]["label"], "echo");
    }
}
