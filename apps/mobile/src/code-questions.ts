export interface CodeQuestion {
  key: string;
  id: string | undefined;
  question: string;
  multiSelect: boolean;
  isSecret: boolean;
  options: { label: string; description: string }[];
}

export interface QuestionChoice {
  selected: string[];
  own: string;
}

export function codeQuestions(input: unknown): CodeQuestion[] {
  if (!input || typeof input !== "object" || !("questions" in input) || !Array.isArray(input.questions)) return [];
  return input.questions.flatMap((raw: unknown): CodeQuestion[] => {
    if (!raw || typeof raw !== "object" || !("question" in raw) || typeof raw.question !== "string") return [];
    const id = "id" in raw && typeof raw.id === "string" ? raw.id : undefined;
    const options = "options" in raw && Array.isArray(raw.options) ? raw.options.flatMap((option: unknown) => {
      if (!option || typeof option !== "object" || !("label" in option) || typeof option.label !== "string") return [];
      return [{ label: option.label, description: "description" in option && typeof option.description === "string" ? option.description : "" }];
    }) : [];
    return [{ key: id ?? raw.question, id, question: raw.question, options, multiSelect: "multiSelect" in raw && raw.multiSelect === true, isSecret: "isSecret" in raw && raw.isSecret === true }];
  });
}

export function answerValues(question: CodeQuestion, choice: QuestionChoice | undefined): string[] {
  if (!choice) return [];
  const own = choice.own.trim();
  return own ? question.multiSelect ? [...choice.selected, own] : [own] : choice.selected;
}

/** Claude expects its question text and one string; Codex expects its stable id and an array. */
export function codeAnswers(questions: readonly CodeQuestion[], choices: Readonly<Record<string, QuestionChoice>>): Record<string, string | string[]> {
  return Object.fromEntries(questions.map((question) => {
    const values = answerValues(question, choices[question.key]);
    return [question.key, question.id === undefined ? values.join(", ") : values];
  }));
}

export function answerReceipt(questions: readonly CodeQuestion[], choices: Readonly<Record<string, QuestionChoice>>): string {
  return questions.map((question) => {
    const answer = question.isSecret ? "Private answer submitted" : answerValues(question, choices[question.key]).join(", ");
    return questions.length === 1 ? answer : `${question.question}\n${answer}`;
  }).join("\n\n");
}
